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
    expect(hello.subscriptions.map((entry) => entry.topic)).toEqual(["events"]);

    socket.send(
      JSON.stringify({
        type: "subscribe",
        topics: [{ topic: "metrics" }],
      }),
    );

    const ack = await inbox.waitFor((message) => message.type === "subscribed");
    expect(ack.topics.map((entry) => entry.topic)).toEqual(["events", "metrics"]);
    const metrics = ack.topics.find((entry) => entry.topic === "metrics");
    expect(metrics.config).toBeTruthy();

    const effects = await readEnvelope(await api.get("/api/v1/effects"));
    const runnableEffect = findRunnableEffect(effects.items, ["Audio Pulse", "Gradient", "Rainbow"]);
    await readEnvelope(await api.post(`/api/v1/effects/${runnableEffect.id}/apply`));

    const effectEvent = await inbox.waitFor(
      (message) => message.type === "event" && message.event === "effect_started",
    );
    expect(effectEvent.event).toBe("effect_started");
  } finally {
    socket.close();
    await api.post("/api/v1/scene/clear");
    await api.dispose();
  }
});
