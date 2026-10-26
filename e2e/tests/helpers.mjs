import { execFile } from "node:child_process";
import { promisify } from "node:util";

import { isolatedEnv } from "../harness/environment.mjs";
import { readRunStateSync } from "../harness/state.mjs";

const execFileAsync = promisify(execFile);

export function getStack() {
  return readRunStateSync();
}

// Scene fences the way the daemon reads and serves them, for fixtures that
// stand in for it. The daemon serves "<run_epoch>:<revision>" as ETag and
// takes it back as If-Match, and a fence naming another run is stale
// whatever its revision. A bare revision is the legacy form a daemon that
// names no runs serves; it checks the revision alone.
const RUN_EPOCH = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const REVISION = /^(?:0|[1-9][0-9]*)$/;

// Parse an If-Match value into { epoch, revision }, epoch null for a bare
// fence. Null when absent or malformed (a list, a weak tag, a bad epoch).
export function parseFence(value) {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  const quoted = trimmed.length >= 2 && trimmed.startsWith('"') && trimmed.endsWith('"');
  const tag = quoted ? trimmed.slice(1, -1) : trimmed;
  const colon = tag.lastIndexOf(":");
  const epoch = colon === -1 ? null : tag.slice(0, colon);
  const revision = colon === -1 ? tag : tag.slice(colon + 1);
  if (epoch !== null && !RUN_EPOCH.test(epoch)) return null;
  return REVISION.test(revision) ? { epoch, revision } : null;
}

// What a fixture records about a write's If-Match: the raw value and its
// parts, revision undefined when the write sent no fence.
export function writeFence(value) {
  const fence = parseFence(value);
  return { fence: value, epoch: fence?.epoch ?? null, revision: fence?.revision };
}

// The ETag a daemon serves for `scene` as it stands.
export function sceneFence(scene) {
  return scene.run_epoch ? `"${scene.run_epoch}:${scene.revision}"` : `"${scene.revision}"`;
}

// Headers every daemon answer carries: the run epoch it was minted in and,
// on a scene answer, the scene's fence.
export function daemonHeaders(scene, { fence = false } = {}) {
  return {
    ...(scene.run_epoch ? { "hypercolor-run-epoch": scene.run_epoch } : {}),
    ...(fence ? { etag: sceneFence(scene) } : {}),
  };
}

// A canonical daemon error answer, ready for route.fulfill.
export function daemonError(scene, status, code, message, details) {
  return {
    status,
    headers: daemonHeaders(scene, { fence: status === 412 }),
    json: {
      error: { code, message, ...(details ? { details } : {}) },
      meta: { api_version: "1.0", request_id: "req_fixture_error", timestamp: "2026-09-10T00:00:00Z" },
    },
  };
}

// The 412 a daemon answers a parsed `fence` that is not current for `scene`.
export function staleFence(scene, fence) {
  const name = (epoch, revision) => (epoch ? `${epoch}:${revision}` : String(revision));
  return daemonError(
    scene, 412, "precondition_failed",
    `version mismatch: expected ${name(fence.epoch, fence.revision)}, current ${name(scene.run_epoch, scene.revision)}`,
    {
      expected: Number(fence.revision),
      current: scene.revision,
      ...(fence.epoch ? { expected_epoch: fence.epoch } : {}),
      ...(scene.run_epoch ? { current_epoch: scene.run_epoch } : {}),
    },
  );
}

// How a daemon answers If-Match `value` on a write fenced against `scene`:
// null when the write may proceed, otherwise the refusal. `required` routes
// refuse a write that sends no fence, or only `*`.
export function refuseFence(scene, value, { required = false } = {}) {
  if (value === undefined || value.trim() === "*") {
    return required
      ? daemonError(scene, 422, "validation_error", "If-Match is required for membership edits")
      : null;
  }
  const fence = parseFence(value);
  if (fence === null) {
    return daemonError(scene, 400, "malformed_request",
      'If-Match must be a scene fence: "<run_epoch>:<revision>" or a bare revision');
  }
  const current = fence.revision === String(scene.revision)
    && (fence.epoch === null || fence.epoch === scene.run_epoch);
  return current ? null : staleFence(scene, fence);
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
    env: isolatedEnv(stack),
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

export function findRunnableHtmlEffect(items) {
  return items.find((item) => item.runnable && item.source === "html") ?? null;
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

export function createMessageInbox(socket) {
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
  socket.on("close", () =>
    failWaiters(new Error("WebSocket closed before the expected message")),
  );

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
