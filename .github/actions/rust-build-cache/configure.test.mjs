import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { cacheKeys, cacheWriter, compilerEnvironment, configure } from './configure.mjs';

const base = {
  shape: 'servo', variant: '', runner: 'Linux-X64', compiler: 'rustc 1.95',
  environment: [['RUSTFLAGS', '-C target-cpu=x86-64']],
  workspaces: [['.', '.cache/hypercolor/target/servo']], revisions: ['revision-a'], locks: ['lock-a'],
};

test('source and lock updates advance immutable entries within compatible restore prefixes', () => {
  const original = cacheKeys(base);
  for (const change of [{ revisions: ['revision-b'] }, { locks: ['lock-b'] }]) {
    const updated = cacheKeys({ ...base, ...change });
    assert.notEqual(updated.key, original.key);
    assert.equal(updated.prefix, original.prefix);
  }
  assert.equal(cacheKeys({ ...base, revisions: ['revision-b'] }).registryKey, original.registryKey);
});

test('different compilation shapes cannot restore each other', () => {
  const original = cacheKeys(base);
  for (const change of [
    { shape: 'native' }, { variant: 'release-linux' }, { runner: 'Linux-ARM64' },
    { compiler: 'rustc 1.96' }, { environment: [['RUSTFLAGS', '-C target-cpu=native']] },
    { workspaces: [['.', 'target']] },
  ]) assert.notEqual(cacheKeys({ ...base, ...change }).prefix, original.prefix);
});

test('compiler environment excludes workflow command lists and irrelevant host settings', () => {
  const common = { RUNNER_OS: 'Linux', CARGO_PROFILE_DEV_OPT_LEVEL: '2', RUSTFLAGS: '-C linker=clang' };
  assert.deepEqual(compilerEnvironment({ ...common, RUST_SHARED_WORKSPACE_ARGS: '--workspace',
    RUST_WINDOWS_WORKSPACE_ARGS: '--exclude daemon', CARGO_TARGET_DIR: '/different/checkout',
    XCODE_VERSION: '26.5', MACOSX_DEPLOYMENT_TARGET: '15.2' }), compilerEnvironment(common));
  for (const [key, value] of [['RUSTFLAGS', 'different'], ['CFLAGS', '-O3'], ['CARGO_PROFILE_DEV_OPT_LEVEL', '1']]) {
    assert.notDeepEqual(compilerEnvironment({ ...common, [key]: value }), compilerEnvironment(common));
  }
  assert.notDeepEqual(compilerEnvironment({ RUNNER_OS: 'macOS', MACOSX_DEPLOYMENT_TARGET: '15.2' }),
    compilerEnvironment({ RUNNER_OS: 'macOS', MACOSX_DEPLOYMENT_TARGET: '15.3' }));
});

test('only the trusted default branch writes shared caches, including explicit save requests', () => {
  for (const [saveIf, ref, branch, event, expected] of [
    ['auto', 'refs/heads/main', 'main', 'push', true],
    ['auto', 'refs/heads/trunk', 'trunk', 'workflow_dispatch', true],
    ['auto', 'refs/heads/main', 'main', 'schedule', true],
    ['false', 'refs/heads/main', 'main', 'push', false],
    ['true', 'refs/pull/1/merge', 'main', 'pull_request', false],
    ['true', 'refs/heads/main', 'main', 'pull_request_target', false],
    ['true', 'refs/tags/v1.0', 'main', 'push', false],
    ['true', 'refs/heads/feature', 'main', 'workflow_dispatch', false],
  ]) assert.equal(cacheWriter(saveIf, ref, branch, event), expected, `${saveIf} ${ref} ${event}`);
  assert.throws(() => cacheWriter('sometimes', 'refs/heads/main', 'main', 'push'));
});

test('nested workspace configuration exports actual paths and tracks both repositories', () => {
  const temp = mkdtempSync(path.join(tmpdir(), 'hypercolor-cache-test-'));
  try {
    const nested = path.join(temp, 'oss');
    mkdirSync(nested);
    execFileSync('git', ['init', '-q', nested]);
    writeFileSync(path.join(nested, 'Cargo.lock'), 'version = 4\n');
    execFileSync('git', ['add', 'Cargo.lock'], { cwd: nested });
    execFileSync('git', ['-c', 'user.name=Cache Test', '-c', 'user.email=cache@example.invalid', 'commit', '-qm', 'fixture'], { cwd: nested });
    const env = {
      GITHUB_WORKSPACE: temp, GITHUB_ENV: path.join(temp, 'env'), GITHUB_SHA: 'outer-a',
      GITHUB_REF: 'refs/heads/main', GITHUB_EVENT_NAME: 'push', CACHE_DEFAULT_BRANCH: 'main',
      RUNNER_OS: 'Linux', RUNNER_ARCH: 'X64', CACHE_WORKSPACES: 'oss -> .cache/target',
      CACHE_SHAPE: 'native-servo', CACHE_DIRECTORIES: 'oss/.cache/mozbuild', CACHE_ON_FAILURE_INPUT: 'true',
    };
    const first = configure(env, 'rustc test fixture');
    assert.ok(first.HYPERCOLOR_BUILD_CACHE_PATHS.includes(path.join(nested, '.cache/target')));
    assert.ok(first.HYPERCOLOR_BUILD_CACHE_PATHS.includes(path.join(nested, '.cache/mozbuild')));
    assert.equal(first.CARGO_INCREMENTAL, '0');
    assert.equal(first.SCCACHE_CACHE_SIZE, '3G');
    assert.equal(first.HYPERCOLOR_CACHE_WRITE, 'true');
    assert.equal(first.HYPERCOLOR_CACHE_SAVE_FAILURE, 'true');
    assert.match(readFileSync(env.GITHUB_ENV, 'utf8'), /HYPERCOLOR_BUILD_CACHE_KEY<</);
    assert.notEqual(configure({ ...env, GITHUB_SHA: 'outer-b' }, 'rustc test fixture').HYPERCOLOR_BUILD_CACHE_KEY, first.HYPERCOLOR_BUILD_CACHE_KEY);
    writeFileSync(path.join(nested, 'source.rs'), '// new source\n');
    execFileSync('git', ['add', 'source.rs'], { cwd: nested });
    execFileSync('git', ['-c', 'user.name=Cache Test', '-c', 'user.email=cache@example.invalid', 'commit', '-qm', 'change'], { cwd: nested });
    const changed = configure(env, 'rustc test fixture');
    assert.notEqual(changed.HYPERCOLOR_BUILD_CACHE_KEY, first.HYPERCOLOR_BUILD_CACHE_KEY);
    assert.equal(changed.HYPERCOLOR_BUILD_CACHE_PREFIX, first.HYPERCOLOR_BUILD_CACHE_PREFIX);
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
});

test('each public cache consumer finalizes, and failure saves retain their ownership gate', () => {
  for (const filename of ['ci.yml', 'servo-cache-warm.yml']) {
    const source = readFileSync(new URL(`../../workflows/${filename}`, import.meta.url), 'utf8');
    const jobs = source.slice(source.indexOf('\njobs:')).split(/\n  [a-zA-Z0-9_-]+:\n/).slice(1);
    for (const job of jobs) {
      const uses = job.match(/uses: \.\/\.github\/actions\/rust-build-cache/g) || [];
      if (uses.length) {
        assert.equal(uses.length, 2);
        assert.match(job, /if: always\(\)\n        uses: \.\/\.github\/actions\/rust-build-cache\n        with:\n          phase: save/);
      }
    }
  }
  const action = readFileSync(new URL('./action.yml', import.meta.url), 'utf8');
  assert.equal((action.match(/env.HYPERCOLOR_CACHE_WRITE == 'true'/g) || []).length, 3);
  assert.equal((action.match(/job.status == 'success' \|\| env.HYPERCOLOR_CACHE_SAVE_FAILURE == 'true'/g) || []).length, 3);
});
