import { test, expect } from "@playwright/test";
import { openControls } from "./control-fixture.mjs";

test("Studio ambient hue updates do not animate scrollbars on non-scrolling chrome", async ({ page }) => {
  await openControls(page);
  const result = await page.evaluate(async () => {
    const root = document.documentElement;
    const previous = root.style.getPropertyValue("--ambient-hue");
    const targets = [
      document.querySelector("#page-search-input"),
      document.querySelector(".resize-handle-line"),
    ];
    const frame = () => new Promise(requestAnimationFrame);
    root.style.setProperty("--ambient-hue", "0");
    await frame();
    await frame();
    root.style.setProperty("--ambient-hue", "180");
    await frame();
    await frame();
    const found = targets.map((element) => ({
      present: element !== null,
      scrollbarTransitions: element?.getAnimations().filter(
        (animation) => animation.transitionProperty === "scrollbar-color",
      ).length,
    }));
    if (previous) root.style.setProperty("--ambient-hue", previous);
    else root.style.removeProperty("--ambient-hue");
    return found;
  });
  expect(result).toEqual([
    { present: true, scrollbarTransitions: 0 },
    { present: true, scrollbarTransitions: 0 },
  ]);
});
