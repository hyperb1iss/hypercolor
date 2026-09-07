import { execFileSync } from 'node:child_process';
import { appendFileSync, mkdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export function startCompilerCache(env = process.env) {
  mkdirSync(env.SCCACHE_DIR, { recursive: true });
  if (env.SCCACHE_SERVER_UDS) mkdirSync(path.dirname(env.SCCACHE_SERVER_UDS), { recursive: true });
  // Check-only jobs never start a compiler. Own the server for the full job,
  // including long gaps between codegen, so finalization always has a live peer.
  execFileSync(env.SCCACHE_PATH || 'sccache', ['--start-server'], {
    env: { ...env, SCCACHE_IDLE_TIMEOUT: '0' }, stdio: 'inherit',
  });
  appendFileSync(env.GITHUB_ENV, 'SCCACHE_IDLE_TIMEOUT=0\nHYPERCOLOR_SCCACHE_STARTED=true\n');
}

export function stopCompilerCache(env = process.env) {
  if (env.HYPERCOLOR_SCCACHE_STARTED !== 'true') return;
  // Shutdown flushes pending writes and returns the final statistics. A lost
  // server or failed flush is a real error and must propagate to the job.
  const stats = execFileSync(env.SCCACHE_PATH || 'sccache', ['--stop-server'], {
    env, encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'],
  });
  process.stdout.write(stats);
  if (env.GITHUB_STEP_SUMMARY) {
    appendFileSync(env.GITHUB_STEP_SUMMARY, `## Compiler cache\n\n\`\`\`text\n${stats}\`\`\`\n`);
  }
  appendFileSync(env.GITHUB_ENV, 'HYPERCOLOR_SCCACHE_STARTED=false\n');
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv[2] === 'start') startCompilerCache();
  else if (process.argv[2] === 'stop') stopCompilerCache();
  else throw new Error('Compiler cache operation must be start or stop');
}
