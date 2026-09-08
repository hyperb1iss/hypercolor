import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { configure } from './configure.mjs';
import { refreshNativeWrappers } from './native-cache.mjs';

test('macOS retains ccache once per path and respects a caller cache location', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'hypercolor-native-cache-'));
  try {
    execFileSync('git', ['init', '-q', root]);
    execFileSync('git', ['-c', 'user.name=Cache Test', '-c', 'user.email=cache@example.invalid',
      'commit', '--allow-empty', '-qm', 'fixture'], { cwd: root });
    const env = { GITHUB_WORKSPACE: root, GITHUB_ENV: path.join(root, 'env'), GITHUB_SHA: 'head',
      CACHE_WORKSPACES: '. -> target\n. -> ./target', RUNNER_OS: 'macOS', RUNNER_ARCH: 'ARM64' };
    const native = path.join(root, '.cache/hypercolor/ccache');
    for (const location of [undefined, path.join(root, 'custom ccache')]) {
      const configured = configure({ ...env, CCACHE_DIR: location,
        CACHE_DIRECTORIES: location || '.cache/hypercolor/ccache' }, 'rustc fixture');
      const paths = configured.HYPERCOLOR_BUILD_CACHE_PATHS.split('\n');
      assert.equal(configured.CCACHE_DIR, location || native);
      assert.equal(paths.filter((entry) => entry === (location || native)).length, 1);
      assert.equal(paths.filter((entry) => entry === path.join(root, 'target')).length, 1);
    }
    for (const os of ['Linux', 'Windows']) {
      const configured = configure({ ...env, RUNNER_OS: os }, 'rustc fixture');
      assert.equal(configured.CCACHE_DIR, undefined);
      assert.ok(!configured.HYPERCOLOR_BUILD_CACHE_PATHS.split('\n').includes(native));
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('only generated nested sccache wrappers are retired after restoration', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'hypercolor-native-wrappers-'));
  try {
    const toolchain = path.join(root, '.cache/hypercolor/toolchain');
    mkdirSync(toolchain, { recursive: true });
    const env = { GITHUB_WORKSPACE: root };
    assert.equal(refreshNativeWrappers(env), 0);
    for (const [name, compiler] of [['cc', 'cc'], ['cxx', 'c++']]) {
      writeFileSync(path.join(toolchain, name),
        `#!/usr/bin/env bash\nexec "/old toolcache/sccache" "$(command -v ${compiler})" "$@"\n`);
    }
    assert.equal(refreshNativeWrappers(env), 2);
    assert.ok(!existsSync(path.join(toolchain, 'cc')));
    assert.ok(!existsSync(path.join(toolchain, 'cxx')));
    const ccache = '#!/usr/bin/env bash\nexec "/opt/homebrew/bin/ccache" "$(command -v cc)" "$@"\n';
    const custom = '#!/usr/bin/env bash\nexec "/usr/bin/sccache" /custom/clang "$@"\n';
    writeFileSync(path.join(toolchain, 'cc'), ccache);
    writeFileSync(path.join(toolchain, 'cxx'), custom);
    assert.equal(refreshNativeWrappers(env), 0);
    assert.equal(readFileSync(path.join(toolchain, 'cc'), 'utf8'), ccache);
    assert.equal(readFileSync(path.join(toolchain, 'cxx'), 'utf8'), custom);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('macOS cache setup precedes configuration and refreshes wrappers after restore', () => {
  const action = readFileSync(new URL('./action.yml', import.meta.url), 'utf8');
  const install = action.indexOf('    - name: Install macOS C/C++ compiler cache\n');
  const configure = action.indexOf('    - name: Configure build cache\n');
  const restore = action.indexOf('    - name: Restore build artifacts and compiler cache\n');
  const refresh = action.indexOf('    - name: Refresh restored macOS compiler wrappers\n');
  const start = action.indexOf('    - name: Start compiler cache server\n');
  assert.ok(install >= 0 && install < configure && configure < restore && restore < refresh && refresh < start);
  assert.match(action.slice(install, configure), /if: inputs.phase == 'restore' && runner.os == 'macOS'/);
  assert.match(action.slice(install, configure), /brew install ccache/);
  assert.match(action.slice(refresh, start), /if: inputs.phase == 'restore' && runner.os == 'macOS'/);
});
