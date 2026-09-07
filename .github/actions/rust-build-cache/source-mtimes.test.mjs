import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, symlinkSync, unlinkSync, utimesSync, writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { captureSourceTimes, restoreSourceTimes } from './source-mtimes.mjs';

function git(root, ...args) {
  const result = spawnSync('git', args, { cwd: root, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
}

function fixture(root) {
  mkdirSync(path.join(root, 'src'), { recursive: true });
  writeFileSync(path.join(root, 'Cargo.toml'), '[package]\nname="freshness_fixture"\nversion="0.1.0"\nedition="2024"\n[workspace]\n');
  writeFileSync(path.join(root, 'src/lib.rs'), 'pub mod value;\n');
  writeFileSync(path.join(root, 'src/value.rs'), 'pub const VALUE: u32 = 1;\n');
  git(root, 'init', '-q');
  git(root, 'add', 'Cargo.toml', 'src');
}

function build(root) {
  return spawnSync('cargo', ['build', '--offline', '-vv'], {
    cwd: root, encoding: 'utf8',
    env: { ...process.env, CARGO_TARGET_DIR: path.join(root, 'target'), CARGO_INCREMENTAL: '0',
      RUSTC_WRAPPER: '', RUSTC_WORKSPACE_WRAPPER: '' },
  });
}

function refreshCheckoutTimes(root) {
  for (const file of ['Cargo.toml', 'src/lib.rs', 'src/value.rs']) {
    const absolute = path.join(root, file);
    const source = readFileSync(absolute);
    unlinkSync(absolute);
    writeFileSync(absolute, source);
  }
}

test('content-qualified timestamps retain Cargo freshness but edits and removals rebuild', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'hypercolor-cargo-freshness-'));
  try {
    fixture(root);
    const snapshot = path.join(root, 'source-times.json');
    let result = build(root);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /Compiling freshness_fixture/);
    assert.equal(captureSourceTimes([root], snapshot), 3);

    refreshCheckoutTimes(root);
    result = build(root);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /Compiling freshness_fixture/);
    captureSourceTimes([root], snapshot);
    refreshCheckoutTimes(root);
    assert.equal(restoreSourceTimes([root], snapshot), 3);
    result = build(root);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /Fresh freshness_fixture/);
    assert.doesNotMatch(result.stderr, /Compiling freshness_fixture/);

    const changed = path.join(root, 'src/value.rs');
    writeFileSync(changed, 'pub const VALUE: u32 = 2;\n');
    const changedTime = statSync(changed).mtimeMs;
    assert.equal(restoreSourceTimes([root], snapshot), 2);
    assert.equal(statSync(changed).mtimeMs, changedTime);
    result = build(root);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /Compiling freshness_fixture/);

    captureSourceTimes([root], snapshot);
    unlinkSync(changed);
    assert.equal(restoreSourceTimes([root], snapshot), 2);
    result = build(root);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /file not found for module `value`/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('nested Git roots restore independently and reject deleted or redirected tracked paths', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'hypercolor-source-times-'));
  try {
    fixture(root);
    const oss = path.join(root, 'oss');
    fixture(oss);
    const roots = [root, oss];
    const snapshot = path.join(root, 'source-times.json');
    assert.equal(captureSourceTimes(roots, snapshot), 6);
    refreshCheckoutTimes(root);
    refreshCheckoutTimes(oss);
    assert.equal(restoreSourceTimes(roots, snapshot), 6);
    git(oss, 'rm', '--cached', 'src/value.rs');
    const removed = path.join(oss, 'src/value.rs');
    utimesSync(removed, 1000, 1000);
    assert.equal(restoreSourceTimes(roots, snapshot), 5);
    assert.equal(statSync(removed).mtimeMs, 1000000);

    const outside = path.join(root, 'outside.rs');
    writeFileSync(outside, 'pub mod value;\n');
    const original = path.join(oss, 'src/lib.rs');
    unlinkSync(original);
    symlinkSync(outside, original);
    assert.equal(restoreSourceTimes(roots, snapshot), 4);
    assert.equal(readFileSync(outside, 'utf8'), 'pub mod value;\n');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
