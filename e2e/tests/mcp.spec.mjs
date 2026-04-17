import { test, expect } from "@playwright/test";

import { createApi, getStack, readJson } from "./helpers.mjs";

const initializeBody = {
  jsonrpc: "2.0",
  id: 1,
  method: "initialize",
  params: {
    protocolVersion: "2025-03-26",
    capabilities: {},
    clientInfo: {
      name: "hypercolor-e2e",
      version: "1.0.0",
    },
  },
};

test("mcp http surface initializes, lists tools/resources, and serves state", async ({
  playwright,
}) => {
  const api = await createApi(playwright);
  const stack = getStack();

  try {
    const initialize = await api.post(`${stack.apiOrigin}/mcp`, {
      headers: {
        accept: "application/json, text/event-stream",
        "content-type": "application/json",
      },
      data: initializeBody,
    });
    expect(initialize.ok()).toBeTruthy();
    const initPayload = await readJson(initialize);
    expect(initPayload.result.protocolVersion).toBeTruthy();

    const tools = await api.post(`${stack.apiOrigin}/mcp`, {
      headers: {
        accept: "application/json, text/event-stream",
        "content-type": "application/json",
      },
      data: {
        jsonrpc: "2.0",
        id: 2,
        method: "tools/list",
      },
    });
    const toolsPayload = await readJson(tools);
    expect(toolsPayload.result.tools.some((tool) => tool.name === "get_status")).toBe(true);

    const callTool = await api.post(`${stack.apiOrigin}/mcp`, {
      headers: {
        accept: "application/json, text/event-stream",
        "content-type": "application/json",
      },
      data: {
        jsonrpc: "2.0",
        id: 3,
        method: "tools/call",
        params: {
          name: "get_status",
          arguments: {},
        },
      },
    });
    const callPayload = await readJson(callTool);
    expect(callPayload.result).toBeTruthy();

    const resources = await api.post(`${stack.apiOrigin}/mcp`, {
      headers: {
        accept: "application/json, text/event-stream",
        "content-type": "application/json",
      },
      data: {
        jsonrpc: "2.0",
        id: 4,
        method: "resources/list",
      },
    });
    const resourcesPayload = await readJson(resources);
    expect(resourcesPayload.result.resources.some((item) => item.uri === "hypercolor://state")).toBe(
      true,
    );

    const readResource = await api.post(`${stack.apiOrigin}/mcp`, {
      headers: {
        accept: "application/json, text/event-stream",
        "content-type": "application/json",
      },
      data: {
        jsonrpc: "2.0",
        id: 5,
        method: "resources/read",
        params: {
          uri: "hypercolor://state",
        },
      },
    });
    const resourcePayload = await readJson(readResource);
    expect(resourcePayload.result.contents.length).toBeGreaterThan(0);
  } finally {
    await api.dispose();
  }
});
