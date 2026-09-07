import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, symlinkSync, unlinkSync, utimesSync, writeFileSync,
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
    assert.equal(restoreSourceTimes(roots, snapshot), 3);
    assert.equal(readFileSync(outside, 'utf8'), 'pub mod value;\n');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

for (const directory of [false, true]) {
  test(`retargeting a tracked ${directory ? 'directory' : 'file'} symlink rebuilds included bytes`, () => {
    const root = mkdtempSync(path.join(tmpdir(), 'hypercolor-symlink-freshness-'));
    try {
      fixture(root);
      unlinkSync(path.join(root, 'src/lib.rs'));
      unlinkSync(path.join(root, 'src/value.rs'));
      const sources = {
        'Cargo.toml': readFileSync(path.join(root, 'Cargo.toml'), 'utf8'),
        'src/main.rs': `fn main() { print!("{}", include_str!("${directory ? 'value/text.txt' : 'value.txt'}")); }\n`,
        [directory ? 'src/a/text.txt' : 'src/a.txt']: 'alpha\n',
        [directory ? 'src/b/text.txt' : 'src/b.txt']: 'beta\n',
      };
      const refresh = () => {
        for (const [relative, body] of Object.entries(sources)) {
          const file = path.join(root, relative);
          mkdirSync(path.dirname(file), { recursive: true });
          writeFileSync(file, body);
        }
      };
      refresh();
      const link = path.join(root, directory ? 'src/value' : 'src/value.txt');
      symlinkSync(directory ? 'a' : 'a.txt', link);
      git(root, 'add', '-A');
      const executable = path.join(root, 'target/debug/freshness_fixture');
      const output = () => {
        const result = spawnSync(executable, [], { encoding: 'utf8' });
        assert.equal(result.status, 0, result.stderr);
        return result.stdout;
      };
      let result = build(root);
      assert.equal(result.status, 0, result.stderr);
      assert.equal(output(), 'alpha\n');
      const snapshot = path.join(root, 'source-times.json');
      assert.equal(captureSourceTimes([root], snapshot), 4);

      refresh();
      assert.equal(restoreSourceTimes([root], snapshot), 4);
      result = build(root);
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stderr, /Fresh freshness_fixture/);

      refresh();
      unlinkSync(link);
      symlinkSync(directory ? 'b' : 'b.txt', link);
      assert.equal(restoreSourceTimes([root], snapshot), 0);
      result = build(root);
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stderr, /Compiling freshness_fixture/);
      assert.equal(output(), 'beta\n');

      unlinkSync(link);
      assert.equal(restoreSourceTimes([root], snapshot), 0);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
}

test('executable mode changes retain checkout timestamps and legacy snapshots are ignored', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'hypercolor-source-mode-'));
  try {
    fixture(root);
    const file = path.join(root, 'src/value.rs');
    const snapshot = path.join(root, 'source-times.json');
    chmodSync(file, 0o644);
    captureSourceTimes([root], snapshot);
    utimesSync(file, 2000, 2000);
    chmodSync(file, 0o755);
    assert.equal(restoreSourceTimes([root], snapshot), 2);
    assert.equal(statSync(file).mtimeMs, 2000000);

    const legacy = JSON.parse(readFileSync(snapshot, 'utf8'));
    legacy.version = 1;
    delete legacy.symlinks;
    writeFileSync(snapshot, JSON.stringify(legacy));
    assert.equal(restoreSourceTimes([root], snapshot), 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
