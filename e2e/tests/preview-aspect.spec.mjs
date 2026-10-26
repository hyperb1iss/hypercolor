import { test, expect } from "@playwright/test";

import { createApi, findRunnableEffect, getStack, readEnvelope } from "./helpers.mjs";

/**
 * Render resolutions the preview must show at their own shape: the old
 * default, 16:9 at two scales, a near-square LED zone, and a 6:1 strip.
 */
const RESOLUTIONS = [
  [320, 200],
  [640, 360],
  [1920, 1080],
  [640, 630],
  [960, 160],
];

const VIEWPORTS = {
  desktop: { width: 1440, height: 900 },
  phone: { width: 390, height: 844 },
};

async function setResolution(api, width, height) {
  for (const [key, value] of [
    ["daemon.canvas_width", width],
    ["daemon.canvas_height", height],
  ]) {
    const response = await api.put(`/api/v1/config/keys/${key}`, {
      headers: { "content-type": "application/json" },
      data: JSON.stringify(value),
    });
    expect(response.ok(), `${key} = ${value}`).toBeTruthy();
  }
  await expect
    .poll(async () => {
      const layout = await readEnvelope(await api.get("/api/v1/layouts/active"));
      return `${layout.canvas_width}x${layout.canvas_height}`;
    })
    .toBe(`${width}x${height}`);
}

/** The shown box's shape against the source it says it shows. */
async function previewShape(page) {
  const box = page.locator("[data-preview-source]").first();
  await expect(box).toBeVisible();
  return await box.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    // The presenter sizes its canvas to each frame it draws, so the canvas's
    // own size proves a frame reached the screen at that shape.
    const canvas = element.querySelector("canvas");
    return {
      source: element.dataset.previewSource,
      fit: element.dataset.previewFit,
      width: rect.width,
      height: rect.height,
      presented: canvas ? [canvas.width, canvas.height] : null,
    };
  });
}

function aspectOf(source) {
  const [width, height] = source.split("x").map(Number);
  return width / height;
}

/**
 * The box must show the configured resolution's shape. The frames it names
 * may be scaled (the dashboard asks for a preview sized to the picture), so
 * the named source is compared by aspect, within a pixel of rounding. A frame
 * rendered before the resize can still land just after it, so the source and
 * the box are read together until both settle on the new shape.
 */
async function expectTrueAspect(page, width, height) {
  const expected = width / height;
  const withinPixel = (aspect, short) =>
    Math.abs(aspect - expected) / expected < 1 / short + 0.002;
  let shape;
  await expect
    .poll(async () => {
      shape = await previewShape(page);
      const [, sourceHeight] = shape.source.split("x").map(Number);
      const presented = shape.presented;
      return (
        presented !== null &&
        presented[0] > 0 &&
        presented[1] > 0 &&
        withinPixel(presented[0] / presented[1], Math.min(presented[1], width)) &&
        withinPixel(aspectOf(shape.source), Math.min(sourceHeight, width)) &&
        withinPixel(shape.width / shape.height, Math.min(shape.width, shape.height))
      );
    }, {
      // A resize re-initializes the presenter; on a busy runner the first
      // frame at the new size can take a few seconds.
      timeout: 20_000,
      message: `the preview should show ${width}x${height} at its own aspect`,
    })
    .toBe(true);
  return shape;
}

test.describe("live preview keeps the source's aspect", () => {
  let api;
  let original;

  test.beforeAll(async ({ playwright }) => {
    api = await createApi(playwright);
    const config = await readEnvelope(await api.get("/api/v1/config"));
    original = [config.daemon.canvas_width, config.daemon.canvas_height];
    // The effects page shows its preview once an effect runs.
    const effects = await readEnvelope(await api.get("/api/v1/effects"));
    const effect = findRunnableEffect(effects.items, ["Gradient", "Rainbow", "Audio Pulse"]);
    await readEnvelope(await api.post(`/api/v1/effects/${effect.id}/apply`));
  });

  test.afterAll(async () => {
    await api.post("/api/v1/scene/clear");
    await setResolution(api, ...original);
    await api.dispose();
  });

  for (const [width, height] of RESOLUTIONS) {
    test(`${width}x${height} on the dashboard, phone, and effects page`, async ({ page }) => {
      const stack = getStack();
      await setResolution(api, width, height);

      await page.setViewportSize(VIEWPORTS.desktop);
      await page.goto(stack.appOrigin, { waitUntil: "networkidle" });
      const dashboard = await expectTrueAspect(page, width, height);
      expect(dashboard.fit).toBe("contain");

      await page.setViewportSize(VIEWPORTS.phone);
      await expectTrueAspect(page, width, height);

      await page.setViewportSize(VIEWPORTS.desktop);
      await page.goto(`${stack.appOrigin}/effects`, { waitUntil: "networkidle" });
      const effects = await expectTrueAspect(page, width, height);
      expect(effects.fit).toBe("fill-width");
      // The 1080p canvas must arrive as a preview sized to this inline box.
      if (width === 1920) {
        expect(effects.presented[0]).toBeLessThanOrEqual(704);
      }
    });
  }

  test("follows a resolution change while the dashboard is open", async ({ page }) => {
    const stack = getStack();
    await setResolution(api, 640, 480);
    await page.setViewportSize(VIEWPORTS.desktop);
    await page.goto(stack.appOrigin, { waitUntil: "networkidle" });
    await expectTrueAspect(page, 640, 480);

    await setResolution(api, 960, 160);
    await expectTrueAspect(page, 960, 160);
    await setResolution(api, 640, 630);
    await expectTrueAspect(page, 640, 630);
  });
});
