import fsp from "node:fs/promises";

import { readRunState, removeRunState } from "./state.mjs";

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
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

export default async function globalTeardown() {
  let state = null;

  try {
    state = await readRunState();
  } catch {
    return;
  }

  await Promise.allSettled([
    terminateProcess(state.webPid),
    terminateProcess(state.daemonPid),
  ]);

  if (!process.env.HYPERCOLOR_E2E_KEEP_RUN_DIR && state.runDir) {
    await fsp.rm(state.runDir, { recursive: true, force: true });
  }

  await removeRunState();
}
