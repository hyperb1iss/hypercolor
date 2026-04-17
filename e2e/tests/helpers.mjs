import { execFile } from "node:child_process";
import { promisify } from "node:util";

import { readRunStateSync } from "../harness/state.mjs";

const execFileAsync = promisify(execFile);

export function getStack() {
  return readRunStateSync();
}

export function uniqueName(prefix) {
  return `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

export async function createApi(playwright) {
  const stack = getStack();
  return await playwright.request.newContext({
    baseURL: stack.apiOrigin,
    extraHTTPHeaders: {
      accept: "application/json",
    },
  });
}

export async function readJson(response) {
  return await response.json();
}

export async function readEnvelope(response) {
  if (!response.ok()) {
    const body = await response.text();
    throw new Error(
      `Request failed with HTTP ${response.status()} ${response.statusText()}\n${body}`,
    );
  }

  const json = await response.json();
  return json.data;
}

export async function callCli(args, { json = true } = {}) {
  const stack = getStack();
  const commandArgs = ["--host", "127.0.0.1", "--port", String(stack.daemonPort)];
  if (json) {
    commandArgs.push("--json");
  }
  commandArgs.push(...args);

  const result = await execFileAsync(stack.cliBinary, commandArgs, {
    cwd: stack.repoRoot,
    env: {
      ...process.env,
      HOME: stack.homeDir,
      XDG_CONFIG_HOME: stack.xdgConfigHome,
      XDG_DATA_HOME: stack.xdgDataHome,
      XDG_CACHE_HOME: stack.xdgCacheHome,
    },
    timeout: 15_000,
  });

  return {
    ...result,
    parsed: json ? JSON.parse(result.stdout) : null,
  };
}

export function buildAttachmentTemplate(templateId, name, ledCount) {
  return {
    id: templateId,
    name,
    vendor: "E2E Vendor",
    category: "strip",
    description: "E2E strip template",
    default_size: {
      width: 0.35,
      height: 0.08,
    },
    topology: {
      type: "strip",
      count: ledCount,
      direction: "left_to_right",
    },
    compatible_slots: [],
    tags: ["e2e", "strip"],
  };
}

export function findEffectByName(items, name) {
  const effect = items.find((item) => item.name === name);
  if (!effect) {
    throw new Error(`Expected to find effect named '${name}'`);
  }
  return effect;
}

export function findRunnableEffect(items, preferredNames = []) {
  for (const name of preferredNames) {
    const preferred = items.find((item) => item.runnable && item.name === name);
    if (preferred) {
      return preferred;
    }
  }

  const runnable = items.find((item) => item.runnable);
  if (!runnable) {
    throw new Error("Expected to find at least one runnable effect");
  }
  return runnable;
}

export function firstControlPayload(activeEffect) {
  const [control] = activeEffect.controls ?? [];
  if (!control) {
    return {};
  }

  return {
    [control.id]: control.default_value,
  };
}
