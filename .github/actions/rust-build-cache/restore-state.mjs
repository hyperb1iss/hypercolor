import { appendFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export function persistRestoreOutcomes(env = process.env) {
  // Restore and save are separate composite invocations, so step outputs
  // must cross the job environment boundary before the finalizer can use them.
  const outcomes = {
    HYPERCOLOR_REGISTRY_CACHE_EXACT_HIT: String(env.CACHE_REGISTRY_EXACT_HIT === 'true'),
    HYPERCOLOR_BUILD_CACHE_EXACT_HIT: String(env.CACHE_BUILD_EXACT_HIT === 'true'),
  };
  appendFileSync(env.GITHUB_ENV, Object.entries(outcomes).map(([key, value]) => `${key}=${value}\n`).join(''));
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) persistRestoreOutcomes();
