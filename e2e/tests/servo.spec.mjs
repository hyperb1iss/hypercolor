import { test, expect } from "@playwright/test";
import { WebSocket } from "ws";

import {
  createApi,
  createMessageInbox,
  findRunnableHtmlEffect,
  getStack,
  readEnvelope,
} from "./helpers.mjs";

const E2E_STACK = process.env.HYPERCOLOR_E2E_STACK ?? "servo";
const METRICS_TIMEOUT_MS = 20_000;

function metricValue(health, key) {
  return Number(health?.[key] ?? 0);
}

function renderedServoFrames(health) {
  return (
    metricValue(health, "servo_render_cpu_frames_total") +
    metricValue(health, "servo_render_gpu_frames_total") +
    metricValue(health, "servo_render_cached_frames_total")
  );
}

function effectHealth(message) {
  return message.data?.effect_health ?? {};
}

test("servo stack renders a bundled HTML effect through Servo", async ({ playwright }) => {
  const stack = getStack();
  const api = await createApi(playwright);

  try {
    const effects = await readEnvelope(await api.get("/api/v1/effects"));
    const htmlEffect = findRunnableHtmlEffect(effects.items);

    if (E2E_STACK === "cpu") {
      test.skip(!htmlEffect, "CPU smoke build disables Servo HTML effects");
    }

    expect(htmlEffect, "expected a runnable HTML effect in the Servo e2e stack").toBeTruthy();

    const wsUrl = `${stack.appOrigin.replace(/^http/, "ws")}/api/v1/ws`;
    const socket = new WebSocket(wsUrl, "hypercolor-v1");
    const inbox = createMessageInbox(socket);

    try {
      await new Promise((resolve, reject) => {
        socket.once("open", resolve);
        socket.once("error", reject);
      });

      await inbox.waitFor((message) => message.type === "hello");
      socket.send(
        JSON.stringify({
          type: "subscribe",
          channels: ["metrics"],
          config: {
            metrics: { interval_ms: 250 },
          },
        }),
      );
      await inbox.waitFor((message) => message.type === "subscribed");

      const before = effectHealth(await inbox.waitFor((message) => message.type === "metrics"));
      const beforePageLoads = metricValue(before, "servo_page_loads_total");
      const beforePageLoadFailures = metricValue(before, "servo_page_load_failures_total");
      const beforeRenderRequests = metricValue(before, "servo_render_requests_total");
      const beforeFrames = renderedServoFrames(before);

      await readEnvelope(await api.post(`/api/v1/effects/${htmlEffect.id}/apply`));
      const active = await readEnvelope(await api.get("/api/v1/effects/active"));
      expect(active.id).toBe(htmlEffect.id);

      const after = await inbox.waitFor((message) => {
        if (message.type !== "metrics") {
          return false;
        }

        const health = effectHealth(message);
        return (
          metricValue(health, "servo_page_loads_total") > beforePageLoads &&
          metricValue(health, "servo_render_requests_total") > beforeRenderRequests &&
          renderedServoFrames(health) > beforeFrames
        );
      }, METRICS_TIMEOUT_MS);

      const health = effectHealth(after);
      expect(metricValue(health, "servo_page_load_failures_total")).toBe(beforePageLoadFailures);
    } finally {
      socket.close();
    }
  } finally {
    await api.post("/api/v1/effects/stop");
    await api.dispose();
  }
});
