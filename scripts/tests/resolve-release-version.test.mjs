import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const script = fileURLToPath(new URL('../resolve-release-version.sh', import.meta.url));

function resolveVersion(packageVersion, requested = '', tag = '') {
  const bin = mkdtempSync(path.join(tmpdir(), 'hypercolor-release-version-'));
  try {
    writeFileSync(path.join(bin, 'cargo'), '#!/bin/sh\nprintf "%s\\n" "$RELEASE_TEST_METADATA"\n', { mode: 0o755 });
    return spawnSync('bash', [script, requested], {
      encoding: 'utf8',
      env: {
        ...process.env,
        PATH: `${bin}:${process.env.PATH}`,
        RELEASE_TEST_METADATA: JSON.stringify({ packages: [{ name: 'hypercolor-daemon', version: packageVersion }] }),
        GITHUB_REF_TYPE: tag ? 'tag' : 'branch',
        GITHUB_REF_NAME: tag || 'main',
      },
    });
  } finally {
    rmSync(bin, { recursive: true, force: true });
  }
}

test('release tags accept both stable and workflow-stamped prerelease versions', () => {
  for (const version of ['0.5.2', '0.5.2-alpha.1', '0.5.2-beta.2', '0.5.2-rc.1']) {
    const result = resolveVersion(version, '', `v${version}`);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout.trim(), version);
  }
});

test('smoke builds retain stable-package CI suffix support', () => {
  const result = resolveVersion('0.5.2');
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout.trim(), '0.5.2-ci.0');
});

test('different release versions and prerelease identities are rejected', () => {
  for (const [packaged, requested] of [
    ['0.5.1', '0.5.2'],
    ['0.5.2-rc.1', '0.5.2-rc.2'],
    ['0.5.2-rc.1', '0.5.2'],
  ]) {
    const result = resolveVersion(packaged, requested);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /does not match Cargo version/);
  }
});

test('the immutable release tag takes precedence over a dispatch input', () => {
  const result = resolveVersion('0.5.2-rc.1', '9.9.9', 'v0.5.2-rc.1');
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout.trim(), '0.5.2-rc.1');
});
