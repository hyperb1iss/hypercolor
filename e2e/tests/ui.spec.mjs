import { test, expect } from "@playwright/test";

import { createApi, findRunnableEffect, getStack, readEnvelope } from "./helpers.mjs";

test("dashboard loads against the live stack", async ({ page }) => {
  const stack = getStack();

  await page.goto(stack.appOrigin, { waitUntil: "networkidle" });

  await expect(page.getByRole("heading", { name: "Dashboard" })).toBeVisible();
  await expect(page.getByRole("img", { name: "Live effect canvas preview" })).toBeVisible();
});

test("effects page can activate an effect through the live UI", async ({ page, playwright }) => {
  const stack = getStack();
  const api = await createApi(playwright);

  try {
    const effects = await readEnvelope(await api.get("/api/v1/effects"));
    const runnableEffect = findRunnableEffect(effects.items, ["Audio Pulse", "Gradient", "Rainbow"]);

    await page.goto(`${stack.appOrigin}/effects`, { waitUntil: "networkidle" });
    await expect(page.getByRole("heading", { name: "Effects" })).toBeVisible();

    await page.getByRole("main").locator("button").filter({ hasText: runnableEffect.name }).first().click();

    await expect(page.getByRole("button", { name: "Pause effect" })).toBeVisible();

    await expect
      .poll(async () => {
        const active = await readEnvelope(await api.get("/api/v1/effects/active"));
        return active.name;
      })
      .toBe(runnableEffect.name);
  } finally {
    await api.post("/api/v1/effects/stop");
    await api.dispose();
  }
});

