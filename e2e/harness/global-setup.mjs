import { spawn } from "node:child_process";
import fs from "node:fs";
import fsp from "node:fs/promises";
import net from "node:net";
import os from "node:os";
import path from "node:path";

import { repoRoot, writeRunState } from "./state.mjs";

const STARTUP_TIMEOUT_MS = 45_000;
const HEALTH_POLL_INTERVAL_MS = 150;
const SEEDED_DISPLAY_NAME = "E2E Preview Simulator";

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function artifactPath(...parts) {
  return path.join(repoRoot, ...parts);
}

function cacheRootPath(...parts) {
  const cacheRoot =
    process.env.HYPERCOLOR_CACHE_DIR ?? path.join(os.homedir(), ".cache", "hypercolor");
  return path.join(cacheRoot, ...parts);
}

function targetPath(...parts) {
  const targetDir = process.env.HYPERCOLOR_E2E_TARGET_DIR ??
    process.env.CARGO_TARGET_DIR ??
    cacheRootPath("target");
  return path.join(targetDir, ...parts);
}

async function assertExists(filePath, label) {
  try {
    await fsp.access(filePath);
  } catch {
    throw new Error(
      `${label} is missing at ${filePath}. Run 'just e2e-build' before starting the suite.`,
    );
  }
}

async function reserveLoopbackPort() {
  return await new Promise((resolve, reject) => {
    const server = net.createServer();
    server.unref();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      const port = typeof address === "object" && address ? address.port : null;
      server.close((error) => {
        if (error) {
          reject(error);
          return;
        }
        if (!port) {
          reject(new Error("Failed to reserve an ephemeral port"));
          return;
        }
        resolve(port);
      });
    });
  });
}

function spawnLoggedProcess(command, args, { cwd, env, logPath }) {
  const logStream = fs.createWriteStream(logPath, { flags: "a" });
  const child = spawn(command, args, {
    cwd,
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });

  child.stdout.pipe(logStream);
  child.stderr.pipe(logStream);
  child.on("exit", () => {
    logStream.end();
  });

  return child;
}

async function terminateProcess(pid) {
  if (!pid) {
    return;
  }

  try {
    process.kill(pid, "SIGTERM");
  } catch (error) {
    if (error.code === "ESRCH") {
      return;
    }
    throw error;
  }

  const deadline = Date.now() + 5_000;
  while (Date.now() < deadline) {
    try {
      process.kill(pid, 0);
      await sleep(100);
    } catch (error) {
      if (error.code === "ESRCH") {
        return;
      }
      throw error;
    }
  }

  try {
    process.kill(pid, "SIGKILL");
  } catch (error) {
    if (error.code !== "ESRCH") {
      throw error;
    }
  }
}

async function waitForJsonHealth(url, label) {
  const deadline = Date.now() + STARTUP_TIMEOUT_MS;

  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok) {
        return;
      }
    } catch {}
    await sleep(HEALTH_POLL_INTERVAL_MS);
  }

  throw new Error(`Timed out waiting for ${label} at ${url}`);
}

async function waitForCondition(check, label) {
  const deadline = Date.now() + STARTUP_TIMEOUT_MS;

  while (Date.now() < deadline) {
    try {
      if (await check()) {
        return;
      }
    } catch {}
    await sleep(HEALTH_POLL_INTERVAL_MS);
  }

  throw new Error(`Timed out waiting for ${label}`);
}

async function waitForHtml(url, label) {
  const deadline = Date.now() + STARTUP_TIMEOUT_MS;

  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok) {
        const body = await response.text();
        if (body.includes("<!DOCTYPE html>") || body.includes("<html")) {
          return;
        }
      }
    } catch {}
    await sleep(HEALTH_POLL_INTERVAL_MS);
  }

  throw new Error(`Timed out waiting for ${label} at ${url}`);
}

function buildConfigToml() {
  return [
    "schema_version = 3",
    "",
    "[mcp]",
    "enabled = true",
    "stateful_mode = false",
    "json_response = true",
    "",
  ].join("\n");
}

async function readEnvelope(url, init) {
  const response = await fetch(url, init);
  if (!response.ok) {
    throw new Error(`Request failed with HTTP ${response.status}: ${await response.text()}`);
  }

  const json = await response.json();
  return json.data;
}

async function seedPreviewSimulator(apiOrigin) {
  const simulator = await readEnvelope(`${apiOrigin}/api/v1/simulators/displays`, {
    method: "POST",
    headers: {
      accept: "application/json",
      "content-type": "application/json",
    },
    body: JSON.stringify({
      name: SEEDED_DISPLAY_NAME,
      width: 480,
      height: 480,
      circular: true,
      enabled: true,
    }),
  });

  await waitForCondition(async () => {
    const displays = await readEnvelope(`${apiOrigin}/api/v1/displays`);
    return Array.isArray(displays) && displays.some((display) => display.id === simulator.id);
  }, "seeded simulator to appear in displays");

  return simulator;
}

export default async function globalSetup() {
  const runDir = await fsp.mkdtemp(path.join(os.tmpdir(), "hypercolor-e2e-"));
  const homeDir = path.join(runDir, "home");
  const xdgConfigHome = path.join(runDir, "config-home");
  const xdgDataHome = path.join(runDir, "data-home");
  const xdgCacheHome = path.join(runDir, "cache-home");
  const daemonLog = path.join(runDir, "daemon.log");
  const webLog = path.join(runDir, "webapp.log");

  const daemonBinary =
    process.env.HYPERCOLOR_E2E_DAEMON_BIN ?? targetPath("debug", "hypercolor-daemon");
  const cliBinary =
    process.env.HYPERCOLOR_E2E_CLI_BIN ?? targetPath("debug", "hypercolor");
  const uiDistDir =
    process.env.HYPERCOLOR_E2E_UI_DIST_DIR ??
    artifactPath("crates", "hypercolor-ui", "dist");
  const webServerScript =
    process.env.HYPERCOLOR_E2E_WEB_SERVER ??
    artifactPath("e2e", "harness", "webapp-server.mjs");

  let daemonPid = null;
  let webPid = null;

  try {
    await Promise.all([
      fsp.mkdir(homeDir, { recursive: true }),
      fsp.mkdir(xdgConfigHome, { recursive: true }),
      fsp.mkdir(xdgDataHome, { recursive: true }),
      fsp.mkdir(xdgCacheHome, { recursive: true }),
      assertExists(daemonBinary, "Daemon binary"),
      assertExists(cliBinary, "CLI binary"),
      assertExists(uiDistDir, "UI dist"),
      assertExists(webServerScript, "Webapp server script"),
    ]);

    const configDir = path.join(xdgConfigHome, "hypercolor");
    await fsp.mkdir(configDir, { recursive: true });
    const configPath = path.join(configDir, "hypercolor.toml");
    await fsp.writeFile(configPath, buildConfigToml(), "utf8");

    const daemonPort = await reserveLoopbackPort();
    const appPort = await reserveLoopbackPort();
    const apiOrigin = `http://127.0.0.1:${daemonPort}`;
    const appOrigin = `http://127.0.0.1:${appPort}`;

    const sharedEnv = {
      ...process.env,
      HYPERCOLOR_E2E: "1",
      NO_COLOR: "1",
      HOME: homeDir,
      XDG_CONFIG_HOME: xdgConfigHome,
      XDG_DATA_HOME: xdgDataHome,
      XDG_CACHE_HOME: xdgCacheHome,
    };

    const daemon = spawnLoggedProcess(
      daemonBinary,
      ["--config", configPath, "--bind", `127.0.0.1:${daemonPort}`, "--log-level", "info"],
      {
        cwd: repoRoot,
        env: sharedEnv,
        logPath: daemonLog,
      },
    );
    daemonPid = daemon.pid ?? null;

    await waitForJsonHealth(`${apiOrigin}/health`, "daemon health");
    const seededDisplay = await seedPreviewSimulator(apiOrigin);

    const web = spawnLoggedProcess(
      process.execPath,
      [
        webServerScript,
        "--port",
        String(appPort),
        "--api-origin",
        apiOrigin,
        "--root",
        uiDistDir,
      ],
      {
        cwd: repoRoot,
        env: sharedEnv,
        logPath: webLog,
      },
    );
    webPid = web.pid ?? null;

    await waitForHtml(appOrigin, "web app");

    await writeRunState({
      runDir,
      repoRoot,
      apiOrigin,
      appOrigin,
      daemonPort,
      appPort,
      configPath,
      homeDir,
      xdgConfigHome,
      xdgDataHome,
      xdgCacheHome,
      seededDisplay,
      daemonBinary,
      cliBinary,
      uiDistDir,
      daemonLog,
      webLog,
      daemonPid,
      webPid,
    });
  } catch (error) {
    await Promise.allSettled([terminateProcess(webPid), terminateProcess(daemonPid)]);
    throw error;
  }
}
