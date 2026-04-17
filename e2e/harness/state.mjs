import fs from "node:fs";
import fsp from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

export const repoRoot = path.resolve(__dirname, "..", "..");
export const statePath =
  process.env.HYPERCOLOR_E2E_STATE_PATH ??
  path.join(os.tmpdir(), `hypercolor-e2e-state-${process.pid}.json`);

export async function writeRunState(state) {
  await fsp.writeFile(statePath, JSON.stringify(state, null, 2), "utf8");
}

export async function readRunState() {
  const raw = await fsp.readFile(statePath, "utf8");
  return JSON.parse(raw);
}

export function readRunStateSync() {
  const raw = fs.readFileSync(statePath, "utf8");
  return JSON.parse(raw);
}

export async function removeRunState() {
  await fsp.rm(statePath, { force: true });
}
