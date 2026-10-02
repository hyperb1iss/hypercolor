import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import {
  cacheKeys, cacheWriter, compilerEnvironment, configure, probeRemoteCache, remoteCompilerCache,
} from './configure.mjs';

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

test('a dependency-scoped archive advances only when the lockfiles do', () => {
  const dependencies = { ...base, scope: 'dependencies' };
  const original = cacheKeys(dependencies);
  assert.equal(cacheKeys({ ...dependencies, revisions: ['revision-b'] }).key, original.key);
  assert.notEqual(cacheKeys({ ...dependencies, locks: ['lock-b'] }).key, original.key);
  assert.equal(original.prefix, cacheKeys(base).prefix);
  // actions/cache prefix-matches the primary key, so the first dependency-scoped
  // restore still finds the revision-scoped entries saved under the same lock.
  assert.ok(cacheKeys(base).key.startsWith(original.key));
  // A failed run saves under the revision key: found by prefix, never an exact hit.
  assert.equal(original.revisionKey, cacheKeys(base).key);
  assert.notEqual(original.revisionKey, original.key);
  assert.throws(() => cacheKeys({ ...base, scope: 'weekly' }), /Invalid archive-key/);
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
      HYPERCOLOR_REGISTRY_CACHE_EXACT_HIT: 'true', HYPERCOLOR_BUILD_CACHE_EXACT_HIT: 'true',
    };
    const first = configure(env, 'rustc test fixture');
    assert.ok(first.HYPERCOLOR_BUILD_CACHE_PATHS.includes(path.join(nested, '.cache/target')));
    assert.ok(first.HYPERCOLOR_BUILD_CACHE_PATHS.includes(path.join(nested, '.cache/mozbuild')));
    assert.equal(first.CARGO_INCREMENTAL, '0');
    assert.equal(first.SCCACHE_CACHE_SIZE, '3G');
    assert.equal(first.HYPERCOLOR_CACHE_WRITE, 'true');
    assert.equal(first.HYPERCOLOR_CACHE_SAVE_FAILURE, 'true');
    assert.equal(first.HYPERCOLOR_REGISTRY_CACHE_EXACT_HIT, 'false');
    assert.equal(first.HYPERCOLOR_BUILD_CACHE_EXACT_HIT, 'false');
    assert.match(readFileSync(env.GITHUB_ENV, 'utf8'), /HYPERCOLOR_BUILD_CACHE_KEY<</);
    assert.notEqual(configure({ ...env, GITHUB_SHA: 'outer-b' }, 'rustc test fixture').HYPERCOLOR_BUILD_CACHE_KEY, first.HYPERCOLOR_BUILD_CACHE_KEY);
    writeFileSync(path.join(nested, 'source.rs'), '// new source\n');
    execFileSync('git', ['add', 'source.rs'], { cwd: nested });
    execFileSync('git', ['-c', 'user.name=Cache Test', '-c', 'user.email=cache@example.invalid', 'commit', '-qm', 'change'], { cwd: nested });
    const changed = configure(env, 'rustc test fixture');
    assert.notEqual(changed.HYPERCOLOR_BUILD_CACHE_KEY, first.HYPERCOLOR_BUILD_CACHE_KEY);
    assert.equal(changed.HYPERCOLOR_BUILD_CACHE_PREFIX, first.HYPERCOLOR_BUILD_CACHE_PREFIX);
    const scoped = { ...env, CACHE_ARCHIVE_KEY: 'dependencies' };
    const dependencyScoped = configure(scoped, 'rustc test fixture');
    assert.equal(configure({ ...scoped, GITHUB_SHA: 'outer-c' }, 'rustc test fixture').HYPERCOLOR_BUILD_CACHE_KEY,
      dependencyScoped.HYPERCOLOR_BUILD_CACHE_KEY);
    assert.equal(dependencyScoped.HYPERCOLOR_CACHE_SAVE_FAILURE, 'true');
    assert.ok(dependencyScoped.HYPERCOLOR_BUILD_CACHE_FAILURE_KEY.startsWith(dependencyScoped.HYPERCOLOR_BUILD_CACHE_KEY));
    assert.notEqual(dependencyScoped.HYPERCOLOR_BUILD_CACHE_FAILURE_KEY, dependencyScoped.HYPERCOLOR_BUILD_CACHE_KEY);
    assert.equal(changed.HYPERCOLOR_BUILD_CACHE_FAILURE_KEY, changed.HYPERCOLOR_BUILD_CACHE_KEY);
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
  // A successful run saves the configured key; a failed one only its revision key.
  const saveKey = action.slice(action.indexOf('- name: Save build artifacts and compiler cache'))
    .match(/\n        key: \$\{\{ (.+) \}\}/)[1];
  const keyFor = (status) => Function('job', 'env', `return ${saveKey}`)(
    { status }, { HYPERCOLOR_BUILD_CACHE_KEY: 'set', HYPERCOLOR_BUILD_CACHE_FAILURE_KEY: 'set-revision' });
  assert.equal(keyFor('success'), 'set');
  assert.equal(keyFor('failure'), 'set-revision');
  assert.equal((action.match(/job.status == 'success' \|\| env.HYPERCOLOR_CACHE_SAVE_FAILURE == 'true'/g) || []).length, 3);
});

const r2 = {
  SCCACHE_R2_ACCESS_KEY_ID: 'a'.repeat(32), SCCACHE_R2_SECRET_ACCESS_KEY: 'b'.repeat(64),
  SCCACHE_R2_BUCKET: 'hypercolor-oss-sccache', SCCACHE_R2_ENDPOINT: 'https://account.r2.cloudflarestorage.com',
  RUNNER_OS: 'Linux', RUNNER_ARCH: 'X64',
};

test('only the trusted cache writer writes the shared compiler cache; everyone else reads', () => {
  const writer = remoteCompilerCache(r2, true).values;
  assert.equal(writer.SCCACHE_S3_RW_MODE, 'READ_WRITE');
  assert.equal(remoteCompilerCache(r2, false).values.SCCACHE_S3_RW_MODE, 'READ_ONLY');
  assert.equal(writer.SCCACHE_BUCKET, 'hypercolor-oss-sccache');
  assert.equal(writer.SCCACHE_ENDPOINT, 'https://account.r2.cloudflarestorage.com');
  assert.equal(writer.SCCACHE_REGION, 'auto');
  assert.equal(writer.SCCACHE_S3_KEY_PREFIX, 'sccache/linux-x64');
  assert.equal(writer.AWS_ACCESS_KEY_ID, r2.SCCACHE_R2_ACCESS_KEY_ID);
  assert.equal(writer.AWS_SECRET_ACCESS_KEY, r2.SCCACHE_R2_SECRET_ACCESS_KEY);
  assert.equal(remoteCompilerCache({ ...r2, SCCACHE_R2_ENDPOINT: 'https://account.r2.cloudflarestorage.com/' }, true)
    .values.SCCACHE_ENDPOINT, 'https://account.r2.cloudflarestorage.com');
});

test('a run without usable R2 settings keeps the local disk cache', () => {
  for (const [change, reason] of [
    [{ SCCACHE_R2_ACCESS_KEY_ID: '' }, /no R2 credentials/],
    [{ SCCACHE_R2_SECRET_ACCESS_KEY: '' }, /no R2 credentials/],
    [{ SCCACHE_R2_BUCKET: '' }, /not configured/],
    [{ SCCACHE_R2_ENDPOINT: '' }, /not configured/],
    [{ SCCACHE_R2_ENDPOINT: 'http://account.r2.cloudflarestorage.com' }, /https origin/],
    [{ SCCACHE_R2_ENDPOINT: 'https://account.r2.cloudflarestorage.com/bucket' }, /https origin/],
  ]) {
    const remote = remoteCompilerCache({ ...r2, ...change }, true);
    assert.equal(remote.values, undefined);
    assert.match(remote.reason, reason);
  }
});

test('the probe sends credentials on stdin and accepts only an authorized answer', () => {
  const calls = [];
  const answer = (stdout) => (command, args, options) => { calls.push({ command, args, options }); return stdout; };
  const settings = { endpoint: 'https://account.r2.cloudflarestorage.com/', bucket: 'cache', keyId: 'key', secret: 'secret' };
  assert.deepEqual(probeRemoteCache(settings, answer('404')), { ok: true, detail: 'HTTP 404' });
  assert.deepEqual(probeRemoteCache(settings, answer('200')), { ok: true, detail: 'HTTP 200' });
  assert.deepEqual(probeRemoteCache(settings, answer('403')), { ok: false, detail: 'HTTP 403' });
  assert.equal(calls[0].command, 'curl');
  assert.ok(calls[0].args.includes('https://account.r2.cloudflarestorage.com/cache/.hypercolor-cache-probe'));
  assert.ok(calls[0].args.includes('--aws-sigv4'));
  assert.ok(!calls[0].args.join(' ').includes('secret'), 'credentials must not appear on the command line');
  assert.equal(calls[0].options.input, 'user = "key:secret"\n');
  const unreachable = () => {
    const error = new Error('Command failed');
    Object.assign(error, { stdout: '000', stderr: 'curl: (7) Failed to connect\n' });
    throw error;
  };
  assert.deepEqual(probeRemoteCache(settings, unreachable), { ok: false, detail: 'curl: (7) Failed to connect' });
});

function gitWorkspace() {
  const temp = mkdtempSync(path.join(tmpdir(), 'hypercolor-r2-test-'));
  execFileSync('git', ['init', '-q', temp]);
  writeFileSync(path.join(temp, 'Cargo.lock'), 'version = 4\n');
  execFileSync('git', ['add', 'Cargo.lock'], { cwd: temp });
  execFileSync('git', ['-c', 'user.name=Cache Test', '-c', 'user.email=cache@example.invalid', 'commit', '-qm', 'fixture'], { cwd: temp });
  return temp;
}

test('configuration enables R2 only after the probe succeeds and exports nothing remote otherwise', () => {
  const temp = gitWorkspace();
  try {
    const env = {
      ...r2, GITHUB_WORKSPACE: temp, GITHUB_ENV: path.join(temp, 'env'), GITHUB_SHA: 'sha',
      GITHUB_REF: 'refs/heads/main', GITHUB_EVENT_NAME: 'push', CACHE_DEFAULT_BRANCH: 'main',
      CACHE_WORKSPACES: '. -> .cache/target',
    };
    const probed = [];
    const enabled = configure(env, 'rustc test fixture', (settings) => { probed.push(settings); return { ok: true, detail: 'HTTP 404' }; });
    assert.equal(probed.length, 1);
    assert.equal(enabled.SCCACHE_BUCKET, 'hypercolor-oss-sccache');
    assert.equal(enabled.SCCACHE_S3_RW_MODE, 'READ_WRITE');
    assert.equal(enabled.HYPERCOLOR_COMPILER_CACHE, 'R2 hypercolor-oss-sccache read-write');
    assert.match(readFileSync(env.GITHUB_ENV, 'utf8'), /SCCACHE_BUCKET<</);

    const pull = configure({ ...env, GITHUB_REF: 'refs/pull/7/merge', GITHUB_EVENT_NAME: 'pull_request', GITHUB_ENV: path.join(temp, 'env-pr') },
      'rustc test fixture', () => ({ ok: true, detail: 'HTTP 404' }));
    assert.equal(pull.SCCACHE_S3_RW_MODE, 'READ_ONLY');

    // An Actions-cache opt-out lane on main still writes the shared compiler cache.
    const optedOut = configure({ ...env, CACHE_SAVE_IF: 'false', GITHUB_ENV: path.join(temp, 'env-opt-out') },
      'rustc test fixture', () => ({ ok: true, detail: 'HTTP 404' }));
    assert.equal(optedOut.HYPERCOLOR_CACHE_WRITE, 'false');
    assert.equal(optedOut.SCCACHE_S3_RW_MODE, 'READ_WRITE');
    const optedOutPull = configure({ ...env, CACHE_SAVE_IF: 'false', GITHUB_REF: 'refs/pull/7/merge',
      GITHUB_EVENT_NAME: 'pull_request', GITHUB_ENV: path.join(temp, 'env-opt-out-pr') },
    'rustc test fixture', () => ({ ok: true, detail: 'HTTP 404' }));
    assert.equal(optedOutPull.SCCACHE_S3_RW_MODE, 'READ_ONLY');

    const down = configure({ ...env, GITHUB_ENV: path.join(temp, 'env-down') }, 'rustc test fixture', () => ({ ok: false, detail: 'HTTP 403' }));
    assert.equal(down.SCCACHE_BUCKET, undefined);
    assert.equal(down.AWS_SECRET_ACCESS_KEY, undefined);
    assert.equal(down.HYPERCOLOR_COMPILER_CACHE, 'local disk (R2 unavailable: HTTP 403)');
    // actions/cache keys restores by a hash of the path list, so R2 and local-disk
    // jobs must save identical paths to keep restoring each other's entries.
    assert.equal(enabled.HYPERCOLOR_BUILD_CACHE_PATHS, down.HYPERCOLOR_BUILD_CACHE_PATHS);
    assert.doesNotMatch(readFileSync(path.join(temp, 'env-down'), 'utf8'), /SCCACHE_BUCKET|AWS_/);

    const fork = configure({ ...env, SCCACHE_R2_ACCESS_KEY_ID: '', SCCACHE_R2_SECRET_ACCESS_KEY: '', GITHUB_ENV: path.join(temp, 'env-fork') },
      'rustc test fixture', () => { throw new Error('a run without credentials must not probe'); });
    assert.equal(fork.SCCACHE_BUCKET, undefined);
    assert.match(fork.HYPERCOLOR_COMPILER_CACHE, /^local disk \(no R2 credentials/);
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
});

test('only main jobs can reach the read-write R2 key, and every cache restore passes the R2 inputs', () => {
  const environment = "    environment:\n      # Only main may hold the read-write R2 key; every other run reads.\n" +
    "      name: ${{ github.ref == 'refs/heads/main' && 'sccache-writer' || '' }}\n      deployment: false\n";
  const inputs = '          r2-access-key-id: ${{ secrets.SCCACHE_R2_ACCESS_KEY_ID }}\n' +
    '          r2-secret-access-key: ${{ secrets.SCCACHE_R2_SECRET_ACCESS_KEY }}\n' +
    '          r2-bucket: ${{ vars.SCCACHE_R2_BUCKET }}\n          r2-endpoint: ${{ vars.SCCACHE_R2_ENDPOINT }}\n';
  let consumers = 0;
  for (const filename of ['ci.yml', 'servo-cache-warm.yml']) {
    const source = readFileSync(new URL(`../../workflows/${filename}`, import.meta.url), 'utf8');
    // Workflow-level env would hand the key to jobs that never compile.
    assert.doesNotMatch(source.slice(0, source.indexOf('\njobs:')), /SCCACHE_R2_/, filename);
    // The action decides read or write mode; a workflow never configures sccache's backend directly.
    assert.doesNotMatch(source, /SCCACHE_(BUCKET|S3_RW_MODE|ENDPOINT):/, filename);
    const jobs = source.slice(source.indexOf('\njobs:')).split(/\n  [a-zA-Z0-9_-]+:\n/).slice(1);
    for (const job of jobs) {
      if (!job.includes('uses: ./.github/actions/rust-build-cache')) {
        assert.doesNotMatch(job, /SCCACHE_R2_|sccache-writer/, `${filename}: only cache consumers see R2 settings`);
        continue;
      }
      consumers += 1;
      assert.ok(job.includes(environment), `${filename}: cache consumer must gate the writer environment`);
      assert.equal(job.split(inputs).length - 1, 1, `${filename}: exactly the restore call passes the R2 inputs`);
      assert.match(job, /uses: \.\/\.github\/actions\/rust-build-cache\n {8}with:\n {10}r2-access-key-id:/);
    }
  }
  assert.equal(consumers, 13);
  const windows = readFileSync(new URL('../../workflows/ci.yml', import.meta.url), 'utf8').match(/^  rust-windows:\n([\s\S]*?)(?=^  [a-z][\w-]*:\n)/m)[1];
  assert.match(windows, /^ {10}archive-key: dependencies$/m, 'the Windows archive is saved per lockfile set');
  const action = readFileSync(new URL('./action.yml', import.meta.url), 'utf8');
  for (const [variable, input] of [['SCCACHE_R2_ACCESS_KEY_ID', 'r2-access-key-id'], ['SCCACHE_R2_SECRET_ACCESS_KEY', 'r2-secret-access-key'],
    ['SCCACHE_R2_BUCKET', 'r2-bucket'], ['SCCACHE_R2_ENDPOINT', 'r2-endpoint']]) {
    assert.ok(action.includes(`        ${variable}: \${{ inputs.${input} }}\n`), variable);
  }
});
