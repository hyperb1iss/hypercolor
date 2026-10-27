import { test, expect } from "@playwright/test";
import { openHistory, FIRST, SECOND } from "./studio-history-fixture.mjs";

const tab = (page, id) => page.locator(`#zone-tab-${id}`);

test("Studio keeps its canvas and tab geometry through rapid selection", async ({ page }) => {
  await openHistory(page);
  await expect(page.locator(".studio-tab-indicator")).toBeVisible();
  await page.evaluate(() => {
    window.motionCanvas = document.querySelector("[data-studio-canvas]");
    window.motionIndicator = document.querySelector(".studio-tab-indicator");
    window.motionTabs = [...document.querySelectorAll("[data-zone-tab]")].map((el) => el.getBoundingClientRect().toJSON());
  });
  for (const id of [SECOND, FIRST, SECOND]) await tab(page, id).click();
  await expect(tab(page, SECOND)).toHaveAttribute("aria-selected", "true");
  await expect.poll(() => page.evaluate(() => {
    const active = document.querySelector('[role="tab"][aria-selected="true"]').parentElement.getBoundingClientRect();
    const line = document.querySelector(".studio-tab-indicator").getBoundingClientRect();
    return Math.abs(line.left - active.left - 8) < 1 && Math.abs(line.width - active.width + 16) < 1;
  })).toBe(true);
  expect(await page.evaluate(() => ({
    canvas: window.motionCanvas === document.querySelector("[data-studio-canvas]"),
    indicator: window.motionIndicator === document.querySelector(".studio-tab-indicator"),
    tabs: [...document.querySelectorAll("[data-zone-tab]")].every((el, i) => {
      const rect = el.getBoundingClientRect();
      return rect.x === window.motionTabs[i].x && rect.width === window.motionTabs[i].width;
    }),
  }))).toEqual({ canvas: true, indicator: true, tabs: true });
});

test("Studio disclosures keep their contents and return composition focus", async ({ page }) => {
  await openHistory(page, { placedStand: true });
  const disclosure = page.locator(`#pane-zone-${SECOND} .studio-disclosure`);
  const toggle = page.locator(`#pane-zone-toggle-${SECOND}`);
  await expect(disclosure).toHaveAttribute("inert", "");
  await toggle.click();
  await expect(disclosure).toHaveAttribute("data-open", "true");
  await disclosure.evaluate((el) => { window.disclosureBody = el.firstElementChild; });
  await toggle.click();
  await expect(disclosure).toHaveAttribute("aria-hidden", "true");
  await toggle.click();
  expect(await disclosure.evaluate((el) => el.firstElementChild === window.disclosureBody)).toBe(true);

  const trigger = page.locator("#studio-composition-trigger");
  const panel = page.locator("#studio-composition-panel");
  await trigger.click();
  expect(await panel.evaluate((el) => getComputedStyle(el).transitionProperty)).toBe("translate");
  await expect(trigger).toHaveAttribute("aria-expanded", "true");
  await expect(panel.getByTitle("Close the composition panel")).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(panel).toHaveAttribute("inert", "");
  await expect(trigger).toBeFocused();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);

  const frames = await trigger.evaluate(async (button) => {
    const panel = document.querySelector("#studio-composition-panel");
    await Promise.all(panel.getAnimations().map((animation) => animation.finished));
    button.click();
    const frames = [];
    for (let frame = 0; frame < 20; frame++) {
      await new Promise(requestAnimationFrame);
      frames.push({ x: panel.getBoundingClientRect().x, scroll: panel.parentElement.scrollLeft });
    }
    return frames;
  });
  expect(frames.every((frame) => frame.scroll === 0)).toBe(true);
  expect(frames.some((frame) => frame.x > frames.at(-1).x + 1 && frame.x < 1600)).toBe(true);
});

test("Studio honors reduced motion in both themes", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await openHistory(page);
  for (const theme of ["dark", "light"]) {
    await page.evaluate((theme) => document.documentElement.dataset.theme = theme, theme);
    await tab(page, SECOND).click();
    expect(await page.evaluate(() => ({
      indicator: getComputedStyle(document.querySelector(".studio-tab-indicator")).transitionDuration,
      panel: getComputedStyle(document.querySelector(".studio-composition-panel")).transitionDuration,
      disclosure: getComputedStyle(document.querySelector(".studio-disclosure")).transitionDuration,
      content: getComputedStyle(document.querySelector(".studio-zone-content")).animationName,
    }))).toEqual({ indicator: "0s", panel: "0s", disclosure: "0s", content: "none" });
    await tab(page, FIRST).click();
  }
});

test("Studio drag feedback stays mounted and clamps continuously at the edge", async ({ page }) => {
  const fixture = await openHistory(page);
  await tab(page, SECOND).click();
  await tab(page, FIRST).click();
  await page.evaluate(async () => {
    await Promise.all([...document.querySelectorAll(".studio-zone-content")].flatMap((el) => el.getAnimations().map((animation) => animation.finished)));
    window.dragSettles = [];
    document.addEventListener("animationstart", (event) => {
      if (event.animationName.startsWith("studio-settle")) window.dragSettles.push(event.animationName);
    });
  });
  const source = page.getByRole("button", { name: "Laptop stand 12 LEDs · Fixture", exact: true });
  const sourceBox = await source.boundingBox();
  await page.mouse.move(sourceBox.x + 30, sourceBox.y + 15);
  await page.mouse.down();
  await page.mouse.move(sourceBox.x + 50, sourceBox.y + 15);
  const ghost = page.locator("[data-studio-drag-ghost]");
  await expect(ghost).toBeVisible();
  await page.evaluate(() => {
    window.dragGhost = document.querySelector("[data-studio-drag-ghost]");
    window.dragPreview = document.querySelector("[data-studio-drop-preview]");
    window.dragPoint = document.querySelector("[data-studio-drop-point]");
  });
  const canvas = await page.locator("[data-studio-canvas]").boundingBox();
  await page.mouse.move(canvas.x + canvas.width / 2, canvas.y + canvas.height / 2);
  await page.mouse.move(canvas.x + canvas.width / 2 + 30, canvas.y + canvas.height / 2 + 30);
  expect(await page.evaluate(() => ({
    ghost: window.dragGhost === document.querySelector("[data-studio-drag-ghost]"),
    preview: window.dragPreview === document.querySelector("[data-studio-drop-preview]"),
    point: window.dragPoint === document.querySelector("[data-studio-drop-point]"),
  }))).toEqual({ ghost: true, preview: true, point: true });
  const positions = [];
  for (const x of [1290, 1310, 1330, 1350, 1370, 1390, 1450, 1590]) {
    await page.mouse.move(x, 650);
    positions.push((await ghost.boundingBox()).x);
  }
  expect(positions.every((x, i) => i === 0 || x >= positions[i - 1])).toBe(true);
  expect(positions.at(-1) + (await ghost.boundingBox()).width).toBeLessThanOrEqual(1592);
  const destination = await tab(page, SECOND).boundingBox();
  await page.mouse.move(destination.x + destination.width / 2, destination.y + destination.height / 2);
  await expect(tab(page, SECOND)).toHaveAttribute("aria-selected", "true");
  await expect(ghost).toBeVisible();
  expect(await page.evaluate(() => window.dragGhost === document.querySelector("[data-studio-drag-ghost]"))).toBe(true);
  await page.keyboard.press("Escape");
  await page.mouse.up();
  await expect(ghost).toHaveCount(0);
  expect(await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve(window.dragSettles)))))).toEqual([]);
  expect(fixture.writes).toEqual([]);
});
