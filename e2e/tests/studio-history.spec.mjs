import { test, expect } from "@playwright/test";
import { openHistory, addStand, dragOutput, FIRST, SECOND } from "./studio-history-fixture.mjs";

const standCard = (page) => page.locator('div.group\\/card').filter({ has: page.getByText("Laptop stand", { exact: true }) });
const anchorCard = (page) => page.locator('div.group\\/card').filter({ has: page.getByText("Anchor light", { exact: true }) });
const moveTo = async (card, page, destination) => {
  await card.getByTitle("Device options").click();
  await card.getByRole("button", { name: "Move to", exact: true }).click();
  await page.getByRole("menuitem", { name: destination, exact: true }).click();
};
const dragTo = async (page, source, target, { toX = 0.5, toY = 0.5 } = {}) => {
  const from = await source.boundingBox();
  const to = await target.boundingBox();
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
  await page.mouse.down();
  const x = to.x + to.width * toX;
  const y = to.y + to.height * toY;
  await page.mouse.move(from.x + from.width / 2 + 20, from.y + from.height / 2, { steps: 4 });
  await page.mouse.move(x, y, { steps: 12 });
  await page.mouse.up();
};
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
  await moveTo(standCard(page), page, "Accent lights");
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
  await page.getByTitle("Offline device options", { exact: true }).click();
  await page.getByRole("button", { name: "Remove from this zone", exact: true }).click();
  await expect.poll(() => fixture.assigned()).toEqual([]);
  await expect(fixture.undo).toBeEnabled();
  await fixture.undo.click();
  await expect.poll(() => fixture.scene.zones[0].members).toEqual(original.members);
  expect(fixture.scene.zones[0].layout).toEqual(original.layout);
  await expect(page.getByTitle("Offline device options", { exact: true })).toBeVisible();
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
  // The drag commits on release; let it land before the remote edit.
  await expect.poll(() => fixture.writes.length).toBe(1);
  await page.waitForLoadState("networkidle");
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

test("Studio commits geometry on release and undo redo commit the previous placement", async ({ page }) => {
  const fixture = await openHistory(page);
  const anchor = page.locator('[data-zone-id="anchor-output"]');
  const original = await anchor.evaluate(geometry);
  const layoutWrites = () => fixture.writes.filter((write) => write.method === "PUT");
  await dragOutput(page, "anchor-output");
  await expect.poll(() => anchor.evaluate(geometry)).not.toEqual(original);
  const moved = await anchor.evaluate(geometry);
  await expect.poll(() => layoutWrites().length).toBe(1);
  expect(layoutWrites()[0].path).toBe(`/api/v1/scene/zones/${FIRST}/layout`);
  const persisted = structuredClone(fixture.scene.zones[0].layout);
  expect(persisted.placements[0].position.x).not.toBe(0.35);
  await expect(page.getByTitle("Save layout changes", { exact: true })).toHaveCount(0);
  await expect(fixture.undo).toBeEnabled();
  await fixture.undo.click();
  await expect.poll(() => anchor.evaluate(geometry)).toEqual(original);
  await expect.poll(() => layoutWrites().length).toBe(2);
  expect(fixture.scene.zones[0].layout.placements[0].position.x).toBe(0.35);
  await expect(fixture.redo).toBeEnabled();
  await fixture.redo.click();
  await expect.poll(() => anchor.evaluate(geometry)).toEqual(moved);
  await expect.poll(() => layoutWrites().length).toBe(3);
  expect(fixture.scene.zones[0].layout).toEqual(persisted);
  expect(fixture.writes).toHaveLength(3);
  expect(fixture.unexpected).toEqual([]);
});

test("Studio removing and restoring a device keeps its committed placement", async ({ page }) => {
  const fixture = await openHistory(page, { placedStand: true });
  const stand = page.locator('[data-zone-id="stand-output"]');
  const original = await stand.evaluate(geometry);
  await dragOutput(page, "stand-output");
  await expect.poll(() => stand.evaluate(geometry)).not.toEqual(original);
  const draft = await stand.evaluate(geometry);
  await standCard(page).getByTitle("Device options").click();
  await standCard(page).getByRole("button", { name: "Remove from this zone", exact: true }).click();
  await expect.poll(() => fixture.assigned()).toEqual([]);
  await expect(stand).toHaveCount(0);
  await fixture.undo.click();
  await expect.poll(() => stand.evaluate(geometry)).toEqual(draft);
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
  await expect.poll(() => fixture.writes.filter((write) => write.method === "POST").length).toBe(1);
  expect(await anchor.evaluate((element) => element.closest("[aria-busy]")?.getAttribute("aria-busy"))).toBe("true");
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

test("Studio adds an Unassigned device to the active tab and can undo it", async ({ page }) => {
  const fixture = await openHistory(page);
  await page.getByRole("tab", { name: "Accent lights", exact: true }).click();
  await expect(page.locator('[data-zone-id="anchor-output"]')).toHaveCount(0);
  await addStand(page, "Accent lights");
  await expect.poll(() => fixture.assigned()).toEqual([{ zone: SECOND, id: "stand-output" }]);
  await expect(page.getByRole("tab", { name: "Accent lights", exact: true })).toHaveAttribute("aria-selected", "true");
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
  await addStand(page);
  await expect.poll(() => fixture.writes.length).toBe(1);
  await page.getByRole("tab", { name: "Accent lights", exact: true }).click();
  release();
  await expect.poll(() => fixture.assigned()).toEqual([{ zone: FIRST, id: "stand-output" }]);
  await expect(page.getByRole("tab", { name: "Accent lights", exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(fixture.undo).toBeEnabled();
  await expect(page.locator('[data-zone-id="stand-output"]')).toHaveCount(0);
  await expect(page.locator('[data-zone-id="anchor-output"]')).toHaveCount(0);
  expect(fixture.unexpected).toEqual([]);
});

test("Studio rejected assignment leaves the device in Unassigned", async ({ page }) => {
  const fixture = await openHistory(page);
  fixture.rejectNext();
  await addStand(page);
  await expect.poll(() => fixture.rejected.length).toBe(1);
  await page.waitForLoadState("networkidle");
  await expect(fixture.undo).toBeDisabled();
  expect(fixture.assigned()).toEqual([]);
  await expect(page.getByTitle("Add to Desk lights", { exact: true })).toBeEnabled();
  expect(fixture.unexpected).toEqual([]);
});

test("Studio pending assignment disables open device menus and Unassigned adds", async ({ page }) => {
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  const fixture = await openHistory(page, { beforeMemberReply: () => gate });
  await anchorCard(page).getByTitle("Device options", { exact: true }).click();
  const move = anchorCard(page).getByRole("button", { name: "Move to", exact: true });
  const remove = anchorCard(page).getByRole("button", { name: "Remove from this zone", exact: true });
  await expect(move).toBeEnabled();
  await addStand(page);
  await expect.poll(() => fixture.writes.length).toBe(1);
  if (!(await move.isVisible())) await anchorCard(page).getByTitle("Device options", { exact: true }).click();
  await expect(move).toBeDisabled();
  await expect(remove).toBeDisabled();
  await expect(page.getByTitle("Add to Desk lights", { exact: true })).toBeDisabled();
  await expect(page.getByRole("tab", { name: "Accent lights", exact: true })).toBeEnabled();
  release();
  await expect.poll(() => fixture.assigned().length).toBe(1);
  if (!(await move.isVisible())) await anchorCard(page).getByTitle("Device options", { exact: true }).click();
  await expect(move).toBeEnabled();
  await expect(remove).toBeEnabled();
  expect(fixture.writes).toHaveLength(1);
  expect(fixture.unexpected).toEqual([]);
});

test("Studio cancels an undo superseded by another zone selection without locking history", async ({ page }) => {
  const fixture = await openHistory(page);
  const anchor = page.locator('[data-zone-id="anchor-output"]');
  const original = await anchor.evaluate(geometry);
  await dragOutput(page, "anchor-output");
  await expect.poll(() => anchor.evaluate(geometry)).not.toEqual(original);
  // An undo pressed while the drag's commit is in flight waits for it, so
  // let the commit land before racing the undo against a tab switch.
  await expect.poll(() => fixture.writes.length).toBe(1);
  await page.waitForLoadState("networkidle");
  await expect(fixture.undo).toBeEnabled();
  const otherZone = page.getByRole("tab", { name: "Accent lights", exact: true });
  // Both handlers run before the reactive provider consumes the queued replay.
  await page.evaluate(({ undo, otherZone }) => { undo.click(); otherZone.click(); }, {
    undo: await fixture.undo.elementHandle(), otherZone: await otherZone.elementHandle(),
  });
  await expect(fixture.undo).toBeEnabled();
  await expect(page.locator('[aria-busy="true"]')).toHaveCount(0);
  await expect(anchor).toHaveCount(0);
  await fixture.undo.click();
  await expect.poll(() => anchor.evaluate(geometry)).toEqual(original);
  await expect(fixture.undo).toBeDisabled();
  await expect(fixture.redo).toBeEnabled();
  expect(fixture.unexpected).toEqual([]);
});

test("Studio drags a device card onto another zone tab and can undo it", async ({ page }) => {
  const fixture = await openHistory(page, { placedStand: true });
  const tab = page.locator(`[data-zone-tab="${SECOND}"]`);
  await dragTo(page, standCard(page).locator("button").first(), tab);
  await expect.poll(() => fixture.assigned()).toEqual([{ zone: SECOND, id: "stand-output" }]);
  // The move lands without switching tabs.
  await expect(page.getByRole("tab", { name: "Desk lights", exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("status").filter({ hasText: "Moved Laptop stand to Accent lights from Desk lights" })).toHaveCount(1);
  await fixture.undo.click();
  await expect.poll(() => fixture.assigned()).toEqual([{ zone: FIRST, id: "stand-output" }]);
  // The live region stops describing the move once it is undone.
  await expect(page.getByRole("status").filter({ hasText: /^Undid: moved .+ back to Desk lights$/ })).toHaveCount(1);
  expect(fixture.rejected).toEqual([]);
  expect(fixture.unexpected).toEqual([]);
});

test("Studio drops an Unassigned device on the canvas at the drop point as one undo step", async ({ page }) => {
  const fixture = await openHistory(page);
  const canvas = page.locator("[data-studio-canvas]");
  const source = page.locator('section[data-pane-unassigned] div.group\\/card').filter({ has: page.getByText("Laptop stand", { exact: true }) });
  await dragTo(page, source.locator("button").first(), canvas, { toX: 0.75, toY: 0.25 });
  await expect.poll(() => fixture.writes.length).toBe(2);
  await expect.poll(() => fixture.assigned()).toEqual([{ zone: FIRST, id: "stand-output" }]);
  const placed = fixture.scene.zones[0].layout.placements.find((placement) => placement.member === "stand-output");
  expect(placed.position.x).toBeCloseTo(0.75, 1);
  expect(placed.position.y).toBeCloseTo(0.25, 1);
  await expect(fixture.undo).toBeEnabled();
  await fixture.undo.click();
  await expect.poll(() => fixture.assigned()).toEqual([]);
  await expect(fixture.undo).toBeDisabled();
  expect(fixture.rejected).toEqual([]);
  expect(fixture.unexpected).toEqual([]);
});
