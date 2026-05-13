import { test, expect } from "@playwright/test";
import { WebSocket } from "ws";

import {
  createApi,
  createMessageInbox,
  findRunnableEffect,
  getStack,
  readEnvelope,
} from "./helpers.mjs";

test("websocket handshake, subscribe ack, and live events flow through the proxy", async ({
  playwright,
}) => {
  const stack = getStack();
  const api = await createApi(playwright);
  const wsUrl = `${stack.appOrigin.replace(/^http/, "ws")}/api/v1/ws`;
  const socket = new WebSocket(wsUrl, "hypercolor-v1");
  const inbox = createMessageInbox(socket);

  try {
    await new Promise((resolve, reject) => {
      socket.once("open", resolve);
      socket.once("error", reject);
    });

    const hello = await inbox.waitFor((message) => message.type === "hello");
    expect(hello.type).toBe("hello");
    expect(hello.version).toBe("1.0");
    expect(hello.capabilities).toContain("events");
    expect(hello.subscriptions).toEqual(["events"]);

    socket.send(
      JSON.stringify({
        type: "subscribe",
        channels: ["metrics"],
      }),
    );

    const ack = await inbox.waitFor((message) => message.type === "subscribed");
    expect(ack.channels).toEqual(["metrics"]);
    expect(ack.config.metrics).toBeTruthy();

    const effects = await readEnvelope(await api.get("/api/v1/effects"));
    const runnableEffect = findRunnableEffect(effects.items, ["Audio Pulse", "Gradient", "Rainbow"]);
    await readEnvelope(await api.post(`/api/v1/effects/${runnableEffect.id}/apply`));

    const effectEvent = await inbox.waitFor(
      (message) =>
        message.type === "event" &&
        ["effect_started", "effect_activated", "effect_changed"].includes(message.event),
    );
    expect(effectEvent.event).toMatch(/effect_/);
  } finally {
    socket.close();
    await api.post("/api/v1/effects/stop");
    await api.dispose();
  }
});
