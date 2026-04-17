import { test, expect } from "@playwright/test";

import { callCli } from "./helpers.mjs";

test("cli can inspect status and run an effect lifecycle against the live daemon", async () => {
  const status = await callCli(["status"]);
  expect(status.parsed.running).toBe(true);

  const effects = await callCli(["effects", "list"]);
  expect(Array.isArray(effects.parsed.items)).toBe(true);
  expect(effects.parsed.items.length).toBeGreaterThan(0);
  const runnableEffect = effects.parsed.items.find((item) => item.runnable);
  expect(runnableEffect).toBeTruthy();

  const activation = await callCli(["effects", "activate", runnableEffect.id]);
  expect(activation.parsed.effect.id).toBe(runnableEffect.id);

  const afterActivation = await callCli(["status"]);
  expect(afterActivation.parsed.active_effect).toBe(runnableEffect.name);

  const stop = await callCli(["effects", "stop"]);
  expect(stop.parsed.stopped).toBe(true);
});
