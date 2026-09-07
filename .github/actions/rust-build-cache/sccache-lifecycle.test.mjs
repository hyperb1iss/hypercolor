import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { startCompilerCache, stopCompilerCache } from './sccache-lifecycle.mjs';

const binary = process.env.SCCACHE_TEST_BINARY || process.env.SCCACHE_PATH || 'sccache';
const available = spawnSync(binary, ['--version']).status === 0;
const realServer = { skip: !available || process.platform === 'win32' };
function fixture() {
  const root = mkdtempSync(path.join(tmpdir(), 'hc-cache-lifecycle-'));
  const env = { ...process.env, SCCACHE_PATH: binary, SCCACHE_DIR: path.join(root, 'cache'),
    SCCACHE_SERVER_UDS: path.join(root, 'server.sock'), SCCACHE_IDLE_TIMEOUT: '0',
    GITHUB_ENV: path.join(root, 'env'), GITHUB_STEP_SUMMARY: path.join(root, 'summary'),
    HYPERCOLOR_SCCACHE_STARTED: '', SCCACHE_CONF: path.join(root, 'config') };
  return { root, env, close() {
    // Cleanup only: the assertions below exercise shutdown without suppressing errors.
    spawnSync(binary, ['--stop-server'], { env });
    rmSync(root, { recursive: true, force: true });
  } };
}

test('failed setup before server startup needs no compiler executable during finalization', () => {
  assert.doesNotThrow(() => stopCompilerCache({ SCCACHE_PATH: '/does-not-exist' }));
});

test('a job with no compiler starts and cleanly finalizes its own empty server', realServer, () => {
  const f = fixture();
  try {
    startCompilerCache(f.env);
    assert.match(readFileSync(f.env.GITHUB_ENV, 'utf8'), /SCCACHE_IDLE_TIMEOUT=0\nHYPERCOLOR_SCCACHE_STARTED=true/);
    stopCompilerCache({ ...f.env, HYPERCOLOR_SCCACHE_STARTED: 'true' });
    assert.match(readFileSync(f.env.GITHUB_STEP_SUMMARY, 'utf8'), /Compile requests\s+0/);
    assert.match(readFileSync(f.env.GITHUB_ENV, 'utf8'), /HYPERCOLOR_SCCACHE_STARTED=false/);
    assert.notEqual(spawnSync(binary, ['--stop-server'], { env: f.env }).status, 0);
  } finally { f.close(); }
});

test('a used server flushes compiled artifacts and reports its real statistics', realServer, () => {
  const f = fixture();
  try {
    startCompilerCache(f.env);
    const source = path.join(f.root, 'fixture.rs');
    writeFileSync(source, 'pub fn value() -> u32 { 42 }\n');
    execFileSync(binary, ['rustc', '--crate-name=fixture', '--crate-type=rlib', '--emit=dep-info,link', source, '--out-dir', f.root], {
      env: { ...f.env, RUSTC_WRAPPER: '', RUSTC_WORKSPACE_WRAPPER: '', CARGO_INCREMENTAL: '0' },
    });
    stopCompilerCache({ ...f.env, HYPERCOLOR_SCCACHE_STARTED: 'true' });
    assert.ok(existsSync(path.join(f.root, 'libfixture.rlib')));
    assert.match(readFileSync(f.env.GITHUB_STEP_SUMMARY, 'utf8'), /Compile requests\s+1/);
    assert.match(readFileSync(f.env.GITHUB_STEP_SUMMARY, 'utf8'), /Cache misses\s+1/);
    rmSync(path.join(f.root, 'libfixture.rlib'));
    startCompilerCache(f.env);
    execFileSync(binary, ['rustc', '--crate-name=fixture', '--crate-type=rlib', '--emit=dep-info,link', source, '--out-dir', f.root], {
      env: { ...f.env, RUSTC_WRAPPER: '', RUSTC_WORKSPACE_WRAPPER: '', CARGO_INCREMENTAL: '0' },
    });
    stopCompilerCache({ ...f.env, HYPERCOLOR_SCCACHE_STARTED: 'true' });
    assert.match(readFileSync(f.env.GITHUB_STEP_SUMMARY, 'utf8'), /Cache hits\s+1/);
    assert.ok(existsSync(path.join(f.root, 'libfixture.rlib')));
  } finally { f.close(); }
});

test('unexpected server loss remains a finalizer failure', realServer, () => {
  const f = fixture();
  try {
    startCompilerCache(f.env);
    execFileSync(binary, ['--stop-server'], { env: f.env });
    assert.throws(() => stopCompilerCache({ ...f.env, HYPERCOLOR_SCCACHE_STARTED: 'true' }));
    assert.doesNotMatch(readFileSync(f.env.GITHUB_ENV, 'utf8'), /STARTED=false/);
  } finally { f.close(); }
});

test('startup failure never publishes server ownership', realServer, () => {
  const f = fixture();
  try {
    writeFileSync(f.env.SCCACHE_CONF, 'invalid = [');
    assert.throws(() => startCompilerCache(f.env));
    assert.equal(existsSync(f.env.GITHUB_ENV), false);
  } finally { f.close(); }
});

test('the action starts after restoration and gates stop on successful ownership', () => {
  const action = readFileSync(new URL('./action.yml', import.meta.url), 'utf8');
  assert.ok(action.indexOf('Restore unchanged source timestamps') < action.indexOf('Start compiler cache server'));
  assert.match(action, /disable_annotations: "true"/);
  assert.match(action, /inputs.phase == 'save' && env.HYPERCOLOR_SCCACHE_STARTED == 'true'/);
  assert.match(action, /sccache-lifecycle.mjs" start/);
  assert.match(action, /sccache-lifecycle.mjs" stop/);
});
