import { test, expect } from "@playwright/test";
import { WebSocket } from "ws";

import { createApi, findRunnableEffect, getStack, readEnvelope } from "./helpers.mjs";

function createMessageInbox(socket) {
  const queue = [];
  const waiters = [];

  function clearWaiter(waiter) {
    clearTimeout(waiter.timeout);
  }

  function removeWaiter(waiter) {
    const index = waiters.indexOf(waiter);
    if (index >= 0) {
      waiters.splice(index, 1);
    }
  }

  function failWaiters(error) {
    for (const waiter of waiters.splice(0)) {
      clearWaiter(waiter);
      waiter.reject(error);
    }
  }

  socket.on("message", (raw) => {
    let parsed = null;

    try {
      parsed = JSON.parse(raw.toString());
    } catch {
      return;
    }

    const waiter = waiters.find((candidate) => candidate.predicate(parsed));
    if (waiter) {
      removeWaiter(waiter);
      clearWaiter(waiter);
      waiter.resolve(parsed);
      return;
    }

    queue.push(parsed);
  });

  socket.on("error", failWaiters);
  socket.on("close", () => failWaiters(new Error("WebSocket closed before the expected message")));

  return {
    waitFor(predicate, timeoutMs = 10_000) {
      const queued = queue.find(predicate);
      if (queued) {
        queue.splice(queue.indexOf(queued), 1);
        return Promise.resolve(queued);
      }

      return new Promise((resolve, reject) => {
        const waiter = {
          predicate,
          resolve,
          reject,
          timeout: setTimeout(() => {
            removeWaiter(waiter);
            reject(new Error("Timed out waiting for a WebSocket message"));
          }, timeoutMs),
        };

        waiters.push(waiter);
      });
    },
  };
}

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
