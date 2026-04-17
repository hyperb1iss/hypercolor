import os from "node:os";
import path from "node:path";

import { defineConfig } from "@playwright/test";

const statePath =
  process.env.HYPERCOLOR_E2E_STATE_PATH ??
  path.join(os.tmpdir(), `hypercolor-e2e-state-${process.pid}.json`);

process.env.HYPERCOLOR_E2E_STATE_PATH = statePath;

export default defineConfig({
  testDir: "./tests",
  fullyParallel: false,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 2 : 0,
  workers: 1,
  timeout: 60_000,
  reporter: process.env.CI
    ? [
        ["line"],
        ["html", { open: "never", outputFolder: "playwright-report" }],
      ]
    : [
        ["list"],
        ["html", { open: "never", outputFolder: "playwright-report" }],
      ],
  outputDir: "test-results",
  use: {
    headless: true,
    trace: "on-first-retry",
    screenshot: "only-on-failure",
    video: process.env.CI ? "retain-on-failure" : "off",
  },
  globalSetup: "./harness/global-setup.mjs",
  globalTeardown: "./harness/global-teardown.mjs",
});
