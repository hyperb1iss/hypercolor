import { test, expect } from "@playwright/test";

import { createApi, findRunnableEffect, getStack, readEnvelope, uniqueName } from "./helpers.mjs";

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

    await expect(page.getByRole("button", { name: "Stop effect" })).toBeVisible();

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

test("displays page can create, edit, and delete a simulator", async ({ page }) => {
  const stack = getStack();
  const simulatorName = uniqueName("E2E Simulator");
  const updatedSimulatorName = `${simulatorName} Updated`;

  await page.goto(`${stack.appOrigin}/displays`, { waitUntil: "networkidle" });
  await expect(page.getByRole("heading", { name: "Displays" })).toBeVisible();

  await page
    .locator("li")
    .filter({ hasText: stack.seededDisplay.name })
    .getByTitle("Edit simulator")
    .click();
  await page.getByRole("button", { name: /delete simulator/i }).click();
  await expect(page.locator("li").filter({ hasText: stack.seededDisplay.name })).toHaveCount(0);

  await page.getByRole("button", { name: /create simulator/i }).click();
  await page.getByLabel("Name").fill(simulatorName);
  await page.getByLabel("Width").fill("320");
  await page.getByLabel("Height").fill("320");
  await page.getByRole("button", { name: /create simulator/i }).last().click();

  await expect(page.getByText(simulatorName)).toBeVisible();

  await page.locator("li").filter({ hasText: simulatorName }).getByTitle("Edit simulator").click();
  await page.getByLabel("Name").fill(updatedSimulatorName);
  await page.getByRole("button", { name: /save simulator/i }).click();

  await expect(page.getByText(updatedSimulatorName)).toBeVisible();

  await page.locator("li").filter({ hasText: updatedSimulatorName }).getByTitle("Edit simulator").click();
  await page.getByRole("button", { name: /delete simulator/i }).click();

  await expect(page.getByText(updatedSimulatorName)).toHaveCount(0);
});
